//! What a scene can show, counted as upper bounds: an `amount` stagger
//! spreads over every member it selects; yoyo with an odd repeat ends on the start values; split words and the
//! character caps count every text a node can show (holds, counters, scrambles, text keys and tween text values);
//! blur sources add up; and the remaining numbers are ranged.

use klipolo_model::contrib::Catalog;
use klipolo_model::scene::{check_scene, Scene, SceneFacts, SceneProblem};

fn checked(json: &str) -> Result<SceneFacts, Vec<SceneProblem>> {
    let s = Scene::from_stored(json.as_bytes()).unwrap_or_else(|e| panic!("{json}: {e:?}"));
    check_scene(&s, Catalog::builtin())
}

fn refused_with(json: &str, code: &str) -> Vec<SceneProblem> {
    let e = checked(json).expect_err(json);
    assert!(e.iter().any(|p| p.code == code), "{code}: {e:?}");
    e
}

fn scene(duration: f64, nodes: &str, timeline: &str) -> String {
    format!(r#"{{"scene":1,"duration":{duration},"nodes":[{nodes}],"timeline":[{timeline}]}}"#)
}

#[test]
fn an_amount_stagger_spreads_over_every_member_it_selects() {
    // 12 split texts of 12 letters and 900 fillers share one second: about a millisecond apart, so every letter of
    // each text blurs at once.
    let nodes = r#"{"type":"repeat","times":12,"children":[{"type":"text","class":"t","text":"ABCDEFGHIJKL","split":"chars"}]},
        {"type":"repeat","times":900,"children":[{"type":"text","class":"t","text":"x"}]}"#;
    let tween = r#"{"to":".t","blur":6,"opacity":0,"dur":0.09,"stagger":{"amount":1}}"#;
    refused_with(&scene(2.0, nodes, tween), "scene.blurringUnits");
    // Without the fillers: 24 letters 1/23 s apart, each blurring for 0.09 s: 3 at once across all twelve texts.
    let pairs = r#"{"type":"repeat","times":12,"children":[{"type":"text","class":"t","text":"AB","split":"chars"}]}"#;
    assert!(checked(&scene(2.0, pairs, tween)).is_ok());
}

#[test]
fn a_tween_counts_its_moving_units_once_across_its_targets() {
    // A title of one text node per word: 16 letters 0.04 s apart, each blurring in for 0.4 s, at most 11 at once.
    let words = ["LEVEL", "UP", "YOUR", "EDITS"]
        .map(|w| format!(r#"{{"type":"text","class":"w","text":"{w}","split":"chars"}}"#))
        .join(",");
    let title = r#"{"from":".w","blur":10,"opacity":0,"dur":0.4,"stagger":{"amount":0.6}}"#;
    assert!(
        checked(&scene(3.0, &words, title)).is_ok(),
        "{:?}",
        checked(&scene(3.0, &words, title))
    );
    // Six rows of a word, 0.05 s apart: 24 letters, 6 or 7 at once.
    let rows = r#"{"type":"repeat","times":6,"children":[{"type":"text","class":"s","text":"SALE","split":"chars"}]}"#;
    let sale = r#"{"from":".s","blur":8,"opacity":0,"dur":0.3,"stagger":0.05}"#;
    assert!(
        checked(&scene(3.0, rows, sale)).is_ok(),
        "{:?}",
        checked(&scene(3.0, rows, sale))
    );
}

#[test]
fn a_tween_text_spreads_a_later_stagger_over_its_letters() {
    // "Hi" becomes 20 letters at 0; a blur-in staggered over 0.2 s then has 20 members, about 0.01 s apart.
    let nodes = r#"{"id":"t","type":"text","text":"Hi","split":"chars"}"#;
    let timeline = r##"{"set":"#t","at":0,"text":"ABCDEFGHIJKLMNOPQRST"},
        {"from":"#t","at":0.5,"blur":8,"opacity":0,"dur":0.3,"stagger":{"amount":0.2}}"##;
    refused_with(&scene(2.0, nodes, timeline), "scene.blurringUnits");
}

#[test]
fn yoyo_with_an_odd_repeat_holds_the_start_values() {
    let letters =
        |id: &str| format!(r#"{{"id":"{id}","type":"text","text":"ABCDEFG","split":"chars"}}"#);
    let two = format!("{},{}", letters("a"), letters("b"));
    let tweens =
        r##"{"from":"#a","dur":0.5,"blur":8,"repeat":1,"yoyo":true},{"to":"#b","at":2,"blur":6}"##;
    refused_with(&scene(4.0, &two, tweens), "scene.blurringUnits");
    // An even repeat ends on the end values, so #a's blur is gone by the time #b's begins.
    let even = tweens.replace(r#""repeat":1"#, r#""repeat":2"#);
    assert!(checked(&scene(4.0, &two, &even)).is_ok());
    // Groups: four ending on a from-blur stay offscreen when a fifth starts at 2 s.
    let groups: Vec<String> = (0..5)
        .map(|i| format!(r#"{{"id":"g{i}","type":"group","children":[{{"type":"rect"}}]}}"#))
        .collect();
    let mut timeline: Vec<String> = (0..4)
        .map(|i| {
            format!(r##"{{"from":"#g{i}","at":0,"dur":0.25,"blur":8,"repeat":1,"yoyo":true}}"##)
        })
        .collect();
    timeline.push(r##"{"to":"#g4","at":2,"dur":0.5,"blur":8}"##.to_owned());
    refused_with(
        &scene(4.0, &groups.join(","), &timeline.join(",")),
        "scene.offscreenGroups",
    );
}

#[test]
fn split_words_count_the_words_of_every_text_the_node_shows() {
    let hold = r#"{"type":"text","text":"Hi","split":"words","blur":8,
        "holds":[{"t":0.5,"text":"a b c d e f g h i j k l m n o p q r s t"}]}"#;
    refused_with(&scene(2.0, hold, ""), "scene.blurringUnits");
    let counter = r#"{"type":"text","text":"0","split":"words","blur":8,
        "count":{"from":0,"to":9,"prefix":"a b c d e f g","suffix":"h i j k l m n"}}"#;
    refused_with(&scene(2.0, counter, ""), "scene.blurringUnits");
}

#[test]
fn text_keys_and_tween_text_count_toward_the_caps_and_units() {
    let long = "x".repeat(1000);
    let keyed = format!(
        r#"{{"id":"t","type":"text","text":"Hi","split":"chars","blur":8,"animate":{{"text":[{{"t":0,"v":"{long}"}}]}}}}"#
    );
    let e = refused_with(&scene(2.0, &keyed, ""), "scene.textTooLong");
    assert!(e.iter().any(|p| p.path == "/nodes/0/text"), "{e:?}");
    let tweened = scene(
        2.0,
        r#"{"id":"t","type":"text","text":"Hi","split":"chars"}"#,
        &format!(r##"{{"set":"#t","at":1,"text":"{}"}}"##, "y".repeat(600)),
    );
    let e = refused_with(&tweened, "scene.textTooLong");
    assert!(e.iter().any(|p| p.path == "/timeline/0/text"), "{e:?}");
    // 13 characters set by a tween, all blurred by the node's own blur.
    let blurred = scene(
        2.0,
        r#"{"id":"t","type":"text","text":"Hi","split":"chars","blur":8}"#,
        r##"{"set":"#t","at":1,"text":"ABCDEFGHIJKLM"}"##,
    );
    refused_with(&blurred, "scene.blurringUnits");
}

#[test]
fn blur_sources_on_one_node_add_up() {
    // Keys blur 8 units at once from the start, a tween 8 more: 16 together, though each alone fits.
    let text = r#"{"id":"t","type":"text","text":"ABCDEFGHIJKLMNOPQRST","split":"chars","stagger":0.125,
        "animate":{"blur":[{"t":0,"v":0},{"t":0.4375,"v":8},{"t":0.875,"v":0}]}}"#;
    let tween = r##"{"to":"#t","at":0,"dur":0.875,"blur":6,"opacity":0,"stagger":{"each":0.125,"from":"end"}}"##;
    let e = refused_with(&scene(4.0, text, tween), "scene.blurringUnits");
    // The advice fits the rule: fewer units at once, the `from` form, or a stagger.
    let advice = &e
        .iter()
        .find(|p| p.code == "scene.blurringUnits")
        .unwrap()
        .message;
    assert!(
        advice.contains("`from`") && advice.contains("stagger") && advice.contains("nothing else"),
        "{advice}"
    );
    assert!(checked(&scene(4.0, text, "")).is_ok(), "the keys alone: 8");
}

#[test]
fn the_remaining_numbers_are_ranged() {
    let refused_at = |nodes: &str, path: &str| {
        let e = checked(&scene(2.0, nodes, "")).expect_err(nodes);
        assert!(e.iter().any(|p| p.path.starts_with(path)), "{path}: {e:?}");
    };
    refused_at(
        r##"{"type":"rect","fill":{"linear":[0,0,1e300,1],"stops":[[0,"#FFFFFF"],[1,"#000000"]]}}"##,
        "/nodes/0/fill",
    );
    refused_at(
        r#"{"type":"polyline","points":[[0,0],[1e300,1]]}"#,
        "/nodes/0/points",
    );
    refused_at(
        r##"{"type":"line","stroke":"#FFFFFF","dash":[1e9]}"##,
        "/nodes/0/dash",
    );
    refused_at(
        r##"{"type":"line","stroke":"#FFFFFF","dash":[4,2,4,2,4]}"##,
        "/nodes/0/dash",
    );
    refused_at(
        r#"{"type":"rect","modifiers":{"opacity":{"type":"wiggle","freq":1,"amp":2}}}"#,
        "/nodes/0/modifiers/opacity",
    );
    refused_at(
        r#"{"type":"rect","modifiers":{"x":{"type":"oscillate","freq":1,"amp":0.1,"phase":1e300}}}"#,
        "/nodes/0/modifiers/x",
    );
    // A bound setting is held to the range at both ends.
    let json = r#"{"scene":1,"duration":2,"params":[{"key":"p","label":"P","type":"float","min":0,"max":100,"default":50}],
        "nodes":[{"type":"group","perspective":"$p"}]}"#;
    let e = checked(json).expect_err("perspective from 0");
    assert!(e.iter().any(|p| p.path == "/nodes/0/perspective"), "{e:?}");
}
