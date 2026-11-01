//! What problems say and how far the loader goes: agent text is escaped and capped, work stops at the problem cap, a
//! refused walk skips the typed parse, a label with a dash takes an offset, table points and generator bindings fit,
//! the schema doesn't offer image files, and `$$` writes a literal `$`.

use klipolo_model::contrib::Catalog;
use klipolo_model::scene::{check_scene, Scene, SceneProblem};

fn checked(json: &str) -> Result<(), Vec<SceneProblem>> {
    let s = Scene::from_stored(json.as_bytes()).unwrap_or_else(|e| panic!("{json}: {e:?}"));
    check_scene(&s, Catalog::builtin()).map(|_| ())
}

#[test]
fn agent_text_in_problems_is_escaped_and_capped() {
    let key = "col\u{202E}our\u{200B}\u{2028}x";
    let json = format!(r#"{{"scene":1,"duration":2,"nodes":[{{"type":"rect","{key}":1}}]}}"#);
    let e = Scene::from_stored(json.as_bytes()).unwrap_err();
    for p in &e {
        for c in ['\u{202E}', '\u{200B}', '\u{2028}'] {
            assert!(!p.path.contains(c) && !p.message.contains(c), "{p:?}");
        }
    }
    assert!(e.iter().any(|p| p.path.contains("\\u{202e}")), "{e:?}");
    // A 100 KB param type comes back short, whoever words it (here serde).
    let long = "x".repeat(100_000);
    let json = format!(
        r#"{{"scene":1,"duration":2,"params":[{{"key":"a","label":"A","type":"{long}"}}],"nodes":[]}}"#
    );
    let e = Scene::from_stored(json.as_bytes()).unwrap_err();
    assert!(
        e.iter()
            .all(|p| p.message.chars().count() <= 600 && p.path.chars().count() <= 256),
        "{e:?}"
    );
}

#[test]
fn work_stops_at_the_problem_cap() {
    // 5,000 unknown keys on one node, and a value of the wrong shape on another. Past the cap problems are only
    // counted; the walk refused, so the typed parse never runs and the bad value isn't among them.
    let keys: Vec<String> = (0..5000).map(|i| format!(r#""bad{i}":1"#)).collect();
    let json = format!(
        r#"{{"scene":1,"duration":2,"nodes":[{{"type":"rect",{}}},{{"type":"rect","x":[1]}}]}}"#,
        keys.join(",")
    );
    let e = Scene::from_stored(json.as_bytes()).unwrap_err();
    assert_eq!(e.len(), 65, "64 problems and a count of the rest");
    assert!(e[..64]
        .iter()
        .all(|p| p.code == "scene.unknownProperty" && p.path.starts_with("/nodes/0/bad")));
    let last = e.last().unwrap();
    assert_eq!((last.path.as_str(), last.code), ("", "scene.moreProblems"));
    assert!(last.message.contains("4936 more"), "{last:?}");
}

#[test]
fn every_invisible_or_format_character_is_escaped() {
    // Tag characters (text an agent reads but a person doesn't), variation selectors, a soft hyphen, the combining
    // grapheme joiner, deprecated format characters, interlinear annotation and a Hangul filler.
    let chars = [
        '\u{E0041}',
        '\u{E0100}',
        '\u{FE0F}',
        '\u{AD}',
        '\u{34F}',
        '\u{206A}',
        '\u{FFF9}',
        '\u{3164}',
        '\u{1D173}',
    ];
    let key: String = std::iter::once('k').chain(chars).collect();
    let json = format!(r#"{{"scene":1,"duration":2,"nodes":[{{"type":"rect","{key}":1}}]}}"#);
    let e = Scene::from_stored(json.as_bytes()).unwrap_err();
    for p in &e {
        for c in chars {
            assert!(
                !p.path.contains(c) && !p.message.contains(c),
                "{c:?} in {p:?}"
            );
        }
    }
    assert!(e.iter().any(|p| p.path.contains("\\u{e0041}")), "{e:?}");
}

#[test]
fn too_many_settings_are_named_before_anything_reads_them() {
    let params: Vec<String> = (0..17)
        .map(|i| format!(r#"{{"key":"p{i}","label":"P","type":"text","default":"","maxLen":10}}"#))
        .collect();
    let json = format!(
        r#"{{"scene":1,"duration":2,"params":[{}],"nodes":[{{"type":"text","text":"$nope"}}]}}"#,
        params.join(",")
    );
    let e = checked(&json).unwrap_err();
    assert_eq!(
        e.iter().map(|p| p.code).collect::<Vec<_>>(),
        ["scene.params"],
        "{e:?}"
    );
}

#[test]
fn an_offset_after_a_label_has_a_sign() {
    let at = |at: &str| {
        format!(
            r##"{{"scene":1,"duration":4,"nodes":[{{"id":"a","type":"rect"}}],"timeline":[
            {{"to":"#a","opacity":0.5,"label":"intro"}},{{"to":"#a","at":"{at}","opacity":1}}]}}"##
        )
    };
    assert_eq!(checked(&at("intro+2")), Ok(()));
    let e = checked(&at("intro2")).unwrap_err();
    assert!(
        e.iter()
            .any(|p| p.path == "/timeline/1/at" && p.code == "scene.at"),
        "{e:?}"
    );
}

#[test]
fn a_refused_walk_skips_the_typed_parse_even_on_a_small_stack() {
    // 30 nodes nested through `child` on rects (which don't take one): the walk refuses the first, and serde never
    // recurses through the rest.
    let mut node = r#"{"type":"rect"}"#.to_owned();
    for _ in 0..29 {
        node = format!(r#"{{"type":"rect","child":{node}}}"#);
    }
    let json = format!(r#"{{"scene":1,"duration":2,"nodes":[{node}]}}"#);
    let worker = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            Scene::from_stored(json.as_bytes())
                .map(|_| ())
                .map_err(|e| e.iter().map(|p| p.code).collect::<Vec<_>>())
        })
        .unwrap();
    let codes = worker.join().unwrap().unwrap_err();
    assert!(codes.contains(&"scene.unknownProperty"), "{codes:?}");
    // Past 1,000 nodes the walk stops, and nothing is built.
    let rects = vec![r#"{"type":"rect"}"#; 1001].join(",");
    let json = format!(r#"{{"scene":1,"duration":2,"nodes":[{rects}]}}"#);
    let e = Scene::from_stored(json.as_bytes()).unwrap_err();
    assert!(e.iter().any(|p| p.code == "scene.tooManyNodes"), "{e:?}");
}

#[test]
fn a_label_with_a_dash_takes_an_offset() {
    let json = r##"{"scene":1,"duration":3,"nodes":[{"id":"a","type":"rect"}],"timeline":[
        {"to":"#a","opacity":0.5,"label":"my-intro"},{"to":"#a","at":"my-intro+=0.2","opacity":1}]}"##;
    assert_eq!(checked(json), Ok(()));
}

#[test]
fn table_points_chart_one_column_and_generator_bindings_fit_its_range() {
    // A table's rows draw as a chart of their first number: one column is enough.
    let one =
        r#"{"key":"data","label":"Data","type":"table","columns":["v"],"maxRows":5,"default":[]}"#;
    let chart = checked(&format!(
        r#"{{"scene":1,"duration":2,"params":[{one}],"nodes":[{{"type":"polyline","points":"$data"}}]}}"#
    ));
    assert_eq!(chart, Ok(()));
    let fill = |param: &str| {
        format!(
            r#"{{"scene":1,"duration":2,"params":[{param}],"nodes":[{{"type":"rect","fill":{{"generator":
            "klipolo.builtin/generator.gradient","params":{{"angle":"$turn"}}}}}}]}}"#
        )
    };
    let wide = r#"{"key":"turn","label":"Turn","type":"float","min":-1000,"max":1000,"default":0}"#;
    let e = checked(&fill(wide)).unwrap_err();
    assert!(
        e.iter()
            .any(|p| p.path == "/nodes/0/fill/params/angle" && p.code == "scene.generator"),
        "{e:?}"
    );
    let fits = r#"{"key":"turn","label":"Turn","type":"float","min":-90,"max":90,"default":0}"#;
    assert_eq!(checked(&fill(fits)), Ok(()));
}

#[test]
fn the_schema_offers_no_image_file_and_a_double_dollar_is_a_literal() {
    let schema = serde_json::to_value(klipolo_model::scene::json_schema())
        .unwrap()
        .to_string();
    assert!(
        !schema.contains("\"file\""),
        "files are refused until packs exist"
    );
    assert!(
        schema.contains("1 to 4") && !schema.contains("at most 16"),
        "a dash has 1 to 4 lengths"
    );
    assert_eq!(klipolo_model::scene::literal("$$Inter"), "$Inter");
    assert_eq!(klipolo_model::scene::literal("Inter"), "Inter");
    let font =
        r#"{"scene":1,"duration":2,"nodes":[{"type":"text","text":"$$5 off","font":"$$Inter"}]}"#;
    assert_eq!(checked(font), Ok(()));
}
