//! `scene@1`: a lower third and a kinetic title load and check, every cap is refused with its name, bindings, unknown
//! keys, selectors, eases and path data are named with their JSON paths, fonts read a `FontRef`'s JSON, stored scenes
//! refuse `svg` nodes, and the stored loader never panics.

use klipolo_model::contrib::Catalog;
use klipolo_model::scene::eval::{eval_into, static_until, SceneFrame};
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::{check_scene, Scene, SceneFont, SceneProblem};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/scenes/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn the_research_lower_third_parses_and_checks() {
    // Per-node keys plus a tween timeline in one scene.
    let s = Scene::from_stored(&fixture("lower-third.json")).unwrap();
    let facts = check_scene(&s, Catalog::builtin()).unwrap();
    assert_eq!(facts.nodes, 5);
    assert!(facts.animates);
}

#[test]
fn scene_caps_are_enforced_with_their_names() {
    let e = check_scene(
        &Scene::from_stored(&fixture("too-many-nodes.json")).unwrap(),
        Catalog::builtin(),
    )
    .unwrap_err();
    assert!(e
        .iter()
        .any(|p| p.code == "scene.tooManyNodes" && p.message.contains("200")));
    let e = Scene::from_stored(&fixture("deep.json")).unwrap_err();
    assert!(e.iter().any(|p| p.code == "scene.tooDeep"));
    let e = Scene::from_stored(&vec![b' '; 300 * 1024]).unwrap_err();
    assert!(e.iter().any(|p| p.code == "scene.tooLarge"));
}

#[test]
fn bindings_must_name_declared_params_of_the_right_kind() {
    let e = check_scene(
        &Scene::from_stored(&fixture("bad-binding.json")).unwrap(),
        Catalog::builtin(),
    )
    .unwrap_err();
    // "$accent" used as a fill but declared as text; "$nope" not declared at all.
    assert!(e
        .iter()
        .any(|p| p.path == "/nodes/0/fill" && p.code == "scene.bindingKind"));
    assert!(e
        .iter()
        .any(|p| p.path == "/nodes/1/text" && p.code == "scene.bindingUnknown"));
}

#[test]
fn unknown_properties_name_their_path_and_a_close_name() {
    let e = Scene::from_stored(&fixture("unknown-prop.json")).unwrap_err();
    let p = e
        .iter()
        .find(|p| p.code == "scene.unknownProperty")
        .unwrap();
    assert_eq!(p.path, "/nodes/1/colour");
    assert!(p.message.contains("did you mean `fill`"));
}

#[test]
fn tween_selectors_and_eases_must_resolve() {
    let mut s = Scene::from_stored(&fixture("kinetic.json")).unwrap();
    s.timeline[0].to = Some(klipolo_model::scene::Selectors::One("#missing".into()));
    s.timeline[1].ease = Some("wobble.out".into());
    let e = check_scene(&s, Catalog::builtin()).unwrap_err();
    assert!(e
        .iter()
        .any(|p| p.code == "scene.selector" && p.path == "/timeline/0/to"));
    assert!(e
        .iter()
        .any(|p| p.code == "scene.ease" && p.path == "/timeline/1/ease"));
}

#[test]
fn svg_path_data_parses_with_arcs_and_refuses_garbage() {
    let cmds = klipolo_model::scene::path::parse_d("M10 10 h20 a5 5 0 0 1 5 5 v10 Z").unwrap();
    assert!(cmds.len() >= 5);
    assert!(klipolo_model::scene::path::parse_d("M10 10 L NaN 5").is_err());
    assert!(klipolo_model::scene::path::parse_d(&"L1 1 ".repeat(5000)).is_err());
}

#[test]
fn a_scene_font_reads_m5bs_font_ref_json_and_the_shorthand() {
    let long = br#"{"scene":1,"duration":2,"nodes":[{"type":"text","text":"Hi","box":[0,0,1,1],
        "font":{"family":"Montserrat","source":{"type":"google","version":"v31"},"weight":700}}]}"#;
    let short = br#"{"scene":1,"duration":2,"nodes":[{"type":"text","text":"Hi","box":[0,0,1,1],"font":"Inter"}]}"#;
    assert!(check_scene(&Scene::from_stored(long).unwrap(), Catalog::builtin()).is_ok());
    let s = Scene::from_stored(short).unwrap();
    assert!(check_scene(&s, Catalog::builtin()).is_ok());
    assert!(
        matches!(s.nodes[0].props.font.as_ref(), Some(SceneFont::Name(f)) if f == "Inter"),
        "the shorthand is kept as written; the engine and scene_fonts resolve it"
    );
}

#[test]
fn stored_scenes_refuse_svg_nodes_and_newer_versions() {
    let svg = br#"{"scene":1,"duration":2,"nodes":[{"type":"svg","src":"logo","box":[0,0,1,1]}]}"#;
    assert!(Scene::from_submission(svg).is_ok());
    assert!(Scene::from_stored(svg)
        .unwrap_err()
        .iter()
        .any(|p| p.code == "scene.svgStored"));
    let newer = br#"{"scene":2,"duration":2,"nodes":[]}"#;
    assert!(Scene::from_stored(newer)
        .unwrap_err()
        .iter()
        .any(|p| p.code == "scene.version" && p.message.contains("newer")));
    let file =
        br#"{"scene":1,"duration":2,"nodes":[{"type":"image","file":"logo.png","box":[0,0,1,1]}]}"#;
    assert!(Scene::from_stored(file)
        .unwrap_err()
        .iter()
        .any(|p| p.code == "scene.imageFile"));
}

#[test]
fn the_stored_scene_loader_never_panics() {
    // A mutation test: a fixed seed, every fixture, 4,000 mutated byte strings (flips, truncations, duplicated
    // spans, swapped numbers for huge ones and NaN-like text). Load, check and compile each; none may panic.
    let mut rng = XorShift64(0x0005_eedd_17a0_u64);
    // The kitchen sinks reach what the others don't: repeats, emitters, images, polylines, masks, spans, tables and
    // generator fills; array selectors, group modifiers, blur and opacity keys on split text, `$i`, settings as
    // burst and rate, pixel keys, `case` and `stagger.from`.
    let fixtures = [
        "lower-third.json",
        "kinetic.json",
        "too-many-nodes.json",
        "bad-binding.json",
        "unknown-prop.json",
        "deep.json",
        "kitchen-sink.json",
        "kitchen-sink-2.json",
    ];
    for name in fixtures {
        let base = fixture(name);
        for _ in 0..(4000 / fixtures.len()) {
            let bytes = mutate(&base, &mut rng);
            let outcome = std::panic::catch_unwind(|| {
                if let Ok(s) = Scene::from_stored(&bytes) {
                    if check_scene(&s, Catalog::builtin()).is_ok() {
                        // A scene that checks compiles, and a layer of it evaluates.
                        let compiled =
                            std::sync::Arc::new(klipolo_model::scene::timeline::compile(&s));
                        let inst = SceneInstance::new(&compiled, &Default::default());
                        let mut frame = SceneFrame::for_instance(&inst);
                        eval_into(&inst, s.duration / 2.0, &mut frame);
                        let _ = static_until(&inst, s.duration / 3.0);
                    }
                }
            });
            assert!(
                outcome.is_ok(),
                "{name}: panicked on {:?}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }
}

/// One scene per cap class, each refused with the cap's code, and the problem names where.
#[test]
fn every_cap_class_is_refused_with_its_code() {
    let stored = |json: &str| Scene::from_stored(json.as_bytes());
    let checked = |json: &str| -> Vec<SceneProblem> {
        let s = stored(json).unwrap_or_else(|e| panic!("{json}: {e:?}"));
        check_scene(&s, Catalog::builtin()).unwrap_err()
    };
    let has =
        |e: &[SceneProblem], code: &str| assert!(e.iter().any(|p| p.code == code), "{code}: {e:?}");
    let scene =
        |nodes: &str, rest: &str| format!(r#"{{"scene":1,"duration":4,"nodes":[{nodes}]{rest}}}"#);

    // The raw JSON's nesting, before any parse.
    let deep_json = format!(
        r#"{{"scene":1,"duration":4,"nodes":[],"x":{}0{}}}"#,
        "[".repeat(40),
        "]".repeat(40)
    );
    has(&stored(&deep_json).unwrap_err(), "scene.tooDeep");
    has(
        &checked(&scene("", "").replace("\"duration\":4", "\"duration\":601")),
        "scene.duration",
    );
    let params: Vec<String> = (0..17)
        .map(|i| {
            format!(r#"{{"key":"p{i}","label":"P","type":"float","min":0,"max":1,"default":0}}"#)
        })
        .collect();
    has(
        &checked(&scene("", &format!(r#","params":[{}]"#, params.join(",")))),
        "scene.params",
    );
    let tweens = vec![r##"{"to":"#a","at":0,"opacity":1}"##; 257].join(",");
    has(
        &checked(&scene(
            r#"{"id":"a","type":"rect"}"#,
            &format!(r#","timeline":[{tweens}]"#),
        )),
        "scene.tooManyTweens",
    );
    let keys: Vec<String> = (0..65)
        .map(|i| format!(r#"{{"t":{},"v":1}}"#, f64::from(i) * 0.05))
        .collect();
    has(
        &checked(&scene(
            &format!(
                r#"{{"type":"rect","animate":{{"opacity":[{}]}}}}"#,
                keys.join(",")
            ),
            "",
        )),
        "scene.tooManyKeys",
    );
    let long = "x".repeat(501);
    let e = checked(&scene(&format!(r#"{{"type":"text","text":"{long}"}}"#), ""));
    assert!(
        e.iter()
            .any(|p| p.code == "scene.textTooLong" && p.path == "/nodes/0/text"),
        "{e:?}"
    );
    let texts = vec![format!(r#"{{"type":"text","text":"{}"}}"#, "y".repeat(450)); 5].join(",");
    has(&checked(&scene(&texts, "")), "scene.textTooLong");
    let repeat = r#"{"type":"repeat","times":600,"children":[{"type":"rect"},{"type":"ellipse"}]}"#;
    has(&checked(&scene(repeat, "")), "scene.tooManyInstances");
    let e = checked(&scene(
        r#"{"type":"emitter","burst":300,"rate":100,"life":[1,3],"child":{"type":"ellipse"}}"#,
        "",
    ));
    assert!(
        e.iter()
            .any(|p| p.code == "scene.tooManyParticles" && p.path == "/nodes/0"),
        "{e:?}"
    );
    let groups = [r#"{"type":"group","blur":4,"children":[{"type":"rect"}]}"#; 5].join(",");
    has(&checked(&scene(&groups, "")), "scene.offscreenGroups");
    // Five groups fading one after another are never offscreen together.
    let fades: Vec<String> = (0..5)
        .map(|i| {
            format!(
                r#"{{"type":"group","children":[{{"type":"rect"}}],"animate":{{"opacity":[{{"t":{},"v":0}},{{"t":{},"v":1}}]}}}}"#,
                f64::from(i) * 0.5,
                f64::from(i) * 0.5 + 0.4
            )
        })
        .collect();
    let s = stored(&scene(&fades.join(","), "")).unwrap();
    assert_eq!(
        check_scene(&s, Catalog::builtin())
            .unwrap()
            .groups_offscreen,
        1
    );
    let blur = r#"{"type":"text","text":"ABCDEFGHIJKLMNOPQRST","split":"chars","animate":{"blur":[{"t":0,"v":8},{"t":0.5,"v":0}]}}"#;
    has(&checked(&scene(blur, "")), "scene.blurringUnits");
    // Staggered 0.1 s apart, the units waiting their turn hold blur 8: all 20 blur at once.
    let staggered = blur.replace(r#""split":"chars","#, r#""split":"chars","stagger":0.1,"#);
    has(&checked(&scene(&staggered, "")), "scene.blurringUnits");
    // Held at opacity 0 while they wait, they aren't drawn, so at most 6 of the 20 blur at once.
    let fading = staggered.replace(
        r#""animate":{"#,
        r#""animate":{"opacity":[{"t":0,"v":0},{"t":0.5,"v":1}],"#,
    );
    assert!(check_scene(&stored(&scene(&fading, "")).unwrap(), Catalog::builtin()).is_ok());
    let d = format!("M0 0 {}", "L1 1 ".repeat(5000));
    let e = checked(&scene(&format!(r#"{{"type":"path","d":"{d}"}}"#), ""));
    assert!(
        e.iter()
            .any(|p| p.code == "scene.path" && p.path == "/nodes/0/d"),
        "{e:?}"
    );
}

#[test]
fn checked_scenes_round_trip_and_mistakes_name_their_place() {
    for name in [
        "lower-third.json",
        "kinetic.json",
        "kitchen-sink.json",
        "kitchen-sink-2.json",
    ] {
        let s = Scene::from_stored(&fixture(name)).unwrap();
        check_scene(&s, Catalog::builtin()).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let again = Scene::from_stored(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(again, s, "{name} round-trips");
    }
    // Unknown names come first (the walk), on their own; then values of the wrong shape (the typed parse).
    let names = br#"{"scene":1,"duration":2,"nodes":[{"type":"rect","fill":{"stops":3}},{"type":"rectangle"}],
        "timeline":[{"to":"rect","opacity":"lots","duration":1}]}"#;
    let e = Scene::from_stored(names).unwrap_err();
    let at = |path: &str, code: &str| e.iter().any(|p| p.path == path && p.code == code);
    assert!(at("/nodes/1/type", "scene.nodeType"), "{e:?}");
    assert!(at("/timeline/0/duration", "scene.unknownProperty"), "{e:?}");
    let shapes = br#"{"scene":1,"duration":2,"nodes":[{"type":"rect","fill":{"stops":3}},{"type":"rect","x":[1]}]}"#;
    let e = Scene::from_stored(shapes).unwrap_err();
    let at = |path: &str, code: &str| e.iter().any(|p| p.path == path && p.code == code);
    assert!(at("/nodes/0/fill", "scene.value"), "{e:?}");
    assert!(at("/nodes/1/x", "scene.value"), "{e:?}");
    // A value of the right shape but the wrong meaning is the checker's: named with its path too.
    let s = Scene::from_stored(
        br##"{"scene":1,"duration":2,"segments":{"in":1.5,"out":1},"nodes":[{"type":"rect","fill":"#12345"}],
            "timeline":[{"to":"rect","at":"later","opacity":1}]}"##,
    )
    .unwrap();
    let e = check_scene(&s, Catalog::builtin()).unwrap_err();
    let at = |path: &str, code: &str| e.iter().any(|p| p.path == path && p.code == code);
    assert!(at("/segments", "scene.segments"), "{e:?}");
    assert!(at("/nodes/0/fill", "scene.color"), "{e:?}");
    assert!(at("/timeline/0/at", "scene.at"), "{e:?}");
    // A fill naming a generator the catalog doesn't know is refused; a known one is listed.
    let fill = |id: &str| {
        format!(
            r#"{{"scene":1,"duration":2,"nodes":[{{"type":"rect","fill":{{"generator":"{id}","params":{{"angle":45}}}}}}]}}"#
        )
    };
    let s = Scene::from_stored(fill("klipolo.builtin/generator.gradient").as_bytes()).unwrap();
    let facts = check_scene(&s, Catalog::builtin()).unwrap();
    assert_eq!(
        facts.generator_fills,
        vec![klipolo_model::ContribId::from(
            "klipolo.builtin/generator.gradient"
        )]
    );
    let s = Scene::from_stored(fill("lib.k3f9q2ma/generator.waves.v1").as_bytes()).unwrap();
    let e = check_scene(&s, Catalog::builtin()).unwrap_err();
    assert!(
        e.iter()
            .any(|p| p.code == "scene.unknownGenerator" && p.path == "/nodes/0/fill/generator"),
        "{e:?}"
    );
}

/// A small seeded generator (Marsaglia's xorshift), so the mutations are the same on every run.
struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// One to three edits: a flipped bit, a truncation, a duplicated span, or a number swapped for a huge one or for
/// NaN-like text.
fn mutate(base: &[u8], rng: &mut XorShift64) -> Vec<u8> {
    const SWAPS: [&str; 7] = [
        "1e308",
        "-1e400",
        "NaN",
        "Infinity",
        "99999999999999999999",
        "-0",
        "4294967296",
    ];
    let mut b = base.to_vec();
    for _ in 0..1 + rng.below(3) {
        if b.is_empty() {
            break;
        }
        let i = rng.below(b.len());
        match rng.below(5) {
            0 => b[i] ^= 1 << rng.below(8),
            1 => b.truncate(i),
            2 => {
                let span = b[i..(i + 1 + rng.below(64)).min(b.len())].to_vec();
                let at = rng.below(b.len());
                b.splice(at..at, span);
            }
            _ => {
                let Some(start) = (i..b.len()).find(|&j| b[j].is_ascii_digit()) else {
                    continue;
                };
                let end = (start..b.len())
                    .find(|&j| !(b[j].is_ascii_digit() || b[j] == b'.'))
                    .unwrap_or(b.len());
                b.splice(start..end, SWAPS[rng.below(SWAPS.len())].bytes());
            }
        }
    }
    b
}
