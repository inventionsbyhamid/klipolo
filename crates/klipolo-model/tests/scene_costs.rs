//! A scene's costs can't be emptied: a repeat counts from a literal or a number setting only; counts take no unbounded
//! binding; modifiers and held blur count; selectors resolve once each; strings and counters are ranged; and a cap
//! names the node that took it over.

use klipolo_model::contrib::Catalog;
use klipolo_model::scene::{check_scene, Scene, SceneFacts, SceneProblem};

fn checked(json: &str) -> Result<SceneFacts, Vec<SceneProblem>> {
    let s = Scene::from_stored(json.as_bytes()).unwrap_or_else(|e| panic!("{json}: {e:?}"));
    check_scene(&s, Catalog::builtin())
}

fn refused(json: &str) -> Vec<SceneProblem> {
    checked(json).expect_err(json)
}

fn has(e: &[SceneProblem], path: &str, code: &str) {
    assert!(
        e.iter().any(|p| p.path == path && p.code == code),
        "{path} {code}: {e:?}"
    );
}

fn scene(params: &str, nodes: &str) -> String {
    format!(r#"{{"scene":1,"duration":2,"params":[{params}],"nodes":[{nodes}]}}"#)
}

const INT_K: &str = r#"{"key":"k","label":"K","type":"int","min":0,"max":5,"default":2}"#;
const TABLE: &str =
    r#"{"key":"data","label":"Data","type":"table","columns":["v"],"maxRows":10,"default":[]}"#;

#[test]
fn a_repeat_counts_only_a_literal_whole_number_or_a_number_setting() {
    // A percentage must not read as no instances, or a million emitters would pass.
    let million = r#"{"type":"repeat","times":"100000%","children":[{"type":"repeat","times":"100000%",
        "children":[{"type":"emitter","burst":500,"child":{"type":"rect"}}]}]}"#;
    has(
        &refused(&scene("", million)),
        "/nodes/0/times",
        "scene.value",
    );
    let inner = |times: &str| {
        format!(
            r#"{{"type":"repeat","rows":"$data","children":[{{"type":"repeat","times":{times},"children":[{{"type":"rect"}}]}}]}}"#
        )
    };
    for (times, code) in [
        (r#""$i""#, "scene.bindingKind"),
        (r#""$row.v0""#, "scene.bindingKind"),
        (r#""$a""#, "scene.bindingKind"),
        (r#""50%""#, "scene.value"),
        ("2.5", "scene.value"),
    ] {
        let angle = r#"{"key":"a","label":"A","type":"angle","default":0}"#;
        let e = refused(&scene(&format!("{TABLE},{angle}"), &inner(times)));
        has(&e, "/nodes/0/children/0/times", code);
    }
    // A literal and an int setting count, at the setting's maximum.
    let facts = checked(&scene(
        INT_K,
        r#"{"type":"repeat","times":"$k","children":[{"type":"repeat","times":3,"children":[{"type":"rect"},{"type":"ellipse"}]}]}"#,
    ))
    .unwrap();
    assert_eq!(
        facts.max_instances,
        5 * 3 * 2,
        "5 × 3 rects and as many ellipses"
    );
    let wide = r#"{"key":"k","label":"K","type":"int","min":0,"max":5000,"default":2}"#;
    has(
        &refused(&scene(
            wide,
            r#"{"type":"repeat","times":"$k","children":[{"type":"rect"}]}"#,
        )),
        "/nodes/0/times",
        "scene.value",
    );
}

#[test]
fn burst_and_rate_take_no_unbounded_binding_and_count_at_their_settings_maximum() {
    let emitter = |burst: &str| {
        format!(
            r#"{{"type":"repeat","rows":"$data","children":[{{"type":"emitter","burst":{burst},"child":{{"type":"rect"}}}}]}}"#
        )
    };
    has(
        &refused(&scene(TABLE, &emitter(r#""$row.v0""#))),
        "/nodes/0/children/0/burst",
        "scene.bindingKind",
    );
    has(
        &refused(&scene(TABLE, &emitter(r#""$i""#))),
        "/nodes/0/children/0/burst",
        "scene.bindingKind",
    );
    let many = r#"{"key":"n","label":"N","type":"int","min":0,"max":600,"default":10}"#;
    let e = refused(&scene(
        many,
        r#"{"type":"emitter","rate":"$n","child":{"type":"rect"}}"#,
    ));
    has(&e, "/nodes/0", "scene.tooManyParticles");
}

#[test]
fn a_group_whose_offscreen_property_has_a_modifier_is_offscreen_all_scene() {
    let group = r#"{"type":"group","modifiers":{"opacity":{"type":"oscillate","freq":1,"amp":0.5}},"children":[{"type":"rect"}]}"#;
    assert_eq!(checked(&scene("", group)).unwrap().groups_offscreen, 1);
    let five = [group; 5].join(",");
    let e = refused(&scene("", &five));
    assert!(e.iter().any(|p| p.code == "scene.offscreenGroups"), "{e:?}");
    // On a split text, a blur modifier blurs every unit all scene.
    let text = r#"{"type":"text","text":"ABCDEFGHIJKLMNOPQRST","split":"chars","modifiers":{"blur":{"type":"wiggle","freq":2,"amp":3}}}"#;
    let e = refused(&scene("", text));
    assert!(e.iter().any(|p| p.code == "scene.blurringUnits"), "{e:?}");
}

#[test]
fn selectors_resolve_once_each_and_a_list_has_at_most_32() {
    let rects = vec![r#"{"type":"rect","class":"a"}"#; 150].join(",");
    let tween = |list: &str| {
        format!(
            r#"{{"scene":1,"duration":2,"nodes":[{rects}],"timeline":[{{"to":[{list}],"opacity":0.5}}]}}"#
        )
    };
    // 32 copies of one selector are one selector.
    assert!(checked(&tween(&vec![r#"".a""#; 32].join(","))).is_ok());
    let e = refused(&tween(&vec![r#"".a""#; 33].join(",")));
    has(&e, "/timeline/0/to", "scene.selector");
}

#[test]
fn held_blur_counts_every_unit_unless_held_invisible() {
    let letters = r#""type":"text","text":"ABCDEFGHIJKLMNOPQRST","split":"chars""#;
    // A staggered blur-out leaves every finished unit blurred.
    let out = format!(
        r##"{{"scene":1,"duration":3,"nodes":[{{"id":"t",{letters}}}],"timeline":[{{"to":"#t","blur":6,"stagger":0.05}}]}}"##
    );
    assert!(refused(&out)
        .iter()
        .any(|p| p.code == "scene.blurringUnits"));
    // Faded out as they blur, they end at opacity 0 and aren't drawn (the renderer culls them before the blur).
    let faded = out.replace(r#""blur":6,"#, r#""blur":6,"opacity":0,"#);
    assert!(checked(&faded).is_ok(), "{:?}", checked(&faded));
    // A staggered from-blur holds every waiting unit blurred, unless it also holds them at opacity 0.
    let from = out.replace(r##""to":"#t""##, r##""from":"#t""##);
    assert!(refused(&from)
        .iter()
        .any(|p| p.code == "scene.blurringUnits"));
    let from_faded = faded.replace(r##""to":"#t""##, r##""from":"#t""##);
    assert!(checked(&from_faded).is_ok(), "{:?}", checked(&from_faded));
}

#[test]
fn pixel_strings_and_counters_are_ranged_and_upper_case_counts_its_length() {
    has(
        &refused(&scene("", r#"{"type":"rect","x":"1e308px"}"#)),
        "/nodes/0/x",
        "scene.value",
    );
    has(
        &refused(&scene("", r#"{"type":"rect","origin":"1e308px 0"}"#)),
        "/nodes/0/origin",
        "scene.value",
    );
    has(
        &refused(&scene(
            "",
            r#"{"type":"text","text":"0","count":{"from":0,"to":1e300}}"#,
        )),
        "/nodes/0/count/to",
        "scene.value",
    );
    // ß upper-cases to SS: 300 of them show 600 characters.
    let sharp = "ß".repeat(300);
    let e = refused(&scene(
        "",
        &format!(r#"{{"type":"text","text":"{sharp}","case":"upper"}}"#),
    ));
    has(&e, "/nodes/0/text", "scene.textTooLong");
    assert!(checked(&scene(
        "",
        &format!(r#"{{"type":"text","text":"{sharp}"}}"#)
    ))
    .is_ok());
    let title = r#"{"key":"title","label":"Title","type":"text","default":"","maxLen":200}"#;
    let e = refused(&scene(
        title,
        r#"{"type":"text","text":"$title","case":"upper"}"#,
    ));
    has(&e, "/nodes/0/text", "scene.textTooLong");
}

#[test]
fn instance_and_text_caps_name_the_node_that_took_them_over() {
    let e = refused(&scene(
        "",
        r#"{"type":"rect"},{"type":"repeat","times":600,"children":[{"type":"rect"},{"type":"ellipse"}]}"#,
    ));
    has(&e, "/nodes/1", "scene.tooManyInstances");
    let texts = vec![format!(r#"{{"type":"text","text":"{}"}}"#, "y".repeat(450)); 5].join(",");
    has(
        &refused(&scene("", &texts)),
        "/nodes/4",
        "scene.textTooLong",
    );
}

/// A scene draws at most 32 different gradients (the renderer's table holds 32), never a silent
/// white past that. Identical gradients count once, wherever and however often they are drawn (a gradient's colours
/// can only come from scene settings, the same in every repeat instance).
#[test]
fn more_than_32_different_gradients_are_refused() {
    let rects = |distinct: bool| -> String {
        (0..33)
            .map(|i| {
                let red = if distinct { i * 7 } else { 0 };
                format!(
                    r##"{{"type":"rect","box":[0,0,0.1,0.1],"fill":{{"linear":[0,0,1,0],"stops":[[0,"#{red:02X}2000"],[1,"#0000FF"]]}}}}"##
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    let check = |nodes: String| {
        let json = format!(r#"{{"scene":1,"duration":2,"nodes":[{nodes}]}}"#);
        let scene = klipolo_model::scene::Scene::from_stored(json.as_bytes()).unwrap();
        klipolo_model::scene::check_scene(&scene, klipolo_model::contrib::Catalog::builtin())
    };
    let problems = check(rects(true)).unwrap_err();
    let p = problems
        .iter()
        .find(|p| p.code == "scene.tooManyGradients")
        .unwrap_or_else(|| panic!("{problems:?}"));
    assert!(
        p.message.contains("32") && p.message.contains("33"),
        "{}",
        p.message
    );
    assert!(check(rects(false)).is_ok(), "one gradient drawn 33 times");
}
