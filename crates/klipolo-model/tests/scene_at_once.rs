//! Split text units blurring at once: a node's held and moving blur together count at most
//! its units; a tween's moving units ramp up and down in nested layers, so tweens that follow each other count
//! the letters actually moving, an upper bound within twice a frame-by-frame simulation; and the refusal names
//! the tween whose stagger to change.

use klipolo_model::contrib::Catalog;
use klipolo_model::scene::{check_scene, Scene, SceneFacts, SceneProblem};

const LETTERS: &str = "ABCDEFGHIJKLMNOPQRST";

fn checked(json: &str) -> Result<SceneFacts, Vec<SceneProblem>> {
    let s = Scene::from_stored(json.as_bytes()).unwrap_or_else(|e| panic!("{json}: {e:?}"));
    check_scene(&s, Catalog::builtin())
}

fn scene(duration: f64, nodes: &[String], timeline: &[String]) -> String {
    format!(
        r#"{{"scene":1,"duration":{duration},"nodes":[{}],"timeline":[{}]}}"#,
        nodes.join(","),
        timeline.join(",")
    )
}

/// The checker's count of units blurring at once. A text of 12 letters blurred all scene by its own blur adds 12 at
/// every moment, so any count is refused, and the refusal says it.
fn at_once(duration: f64, nodes: &[String], timeline: &[String]) -> usize {
    let mut padded = vec![
        r#"{"id":"pad","type":"text","text":"ABCDEFGHIJKL","split":"chars","blur":8}"#.to_owned(),
    ];
    padded.extend_from_slice(nodes);
    let json = scene(duration, &padded, timeline);
    let e = checked(&json).expect_err(&json);
    assert!(e.len() == 1 && e[0].code == "scene.blurringUnits", "{e:?}");
    let count = e[0]
        .message
        .split_once(", ")
        .and_then(|(_, rest)| rest.split_once(' '))
        .and_then(|(n, _)| n.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("{}", e[0].message));
    count - 12
}

fn one(json: &str) -> Vec<String> {
    vec![json.to_owned()]
}

#[test]
fn a_nodes_held_and_moving_blur_count_its_units_once() {
    // "SUBSCRIBE" blurs in with `to` from a blurred, invisible rest. Its own blur counts all 9 letters all scene (the
    // tween writes the opacity), so the tween moving them adds none: 9, not 18.
    let subscribe =
        one(r#"{"id":"t","type":"text","text":"SUBSCRIBE","split":"chars","blur":10,"opacity":0}"#);
    let to = one(r##"{"to":"#t","blur":0,"opacity":1,"dur":0.4,"stagger":0.05}"##);
    assert!(
        checked(&scene(3.0, &subscribe, &to)).is_ok(),
        "{:?}",
        checked(&scene(3.0, &subscribe, &to))
    );
    assert_eq!(at_once(3.0, &subscribe, &to), 9);
    // A blur wiggle counts all 8 letters of "GLITCHED"; its blur-in adds none: 8, not 16.
    let glitched = one(
        r#"{"id":"t","type":"text","text":"GLITCHED","split":"chars","modifiers":{"blur":{"type":"wiggle","freq":2,"amp":3}}}"#,
    );
    let from = one(r##"{"from":"#t","blur":10,"opacity":0,"dur":0.5,"stagger":0.05}"##);
    assert!(
        checked(&scene(3.0, &glitched, &from)).is_ok(),
        "{:?}",
        checked(&scene(3.0, &glitched, &from))
    );
    assert_eq!(at_once(3.0, &glitched, &from), 8);
    // 13 letters stay refused: a split text's own blur counts every unit.
    let long = one(
        r#"{"id":"t","type":"text","text":"SUBSCRIBENOW!","split":"chars","blur":10,"opacity":0}"#,
    );
    assert!(checked(&scene(3.0, &long, &to)).is_err());
}

#[test]
fn one_tween_per_word_counts_the_letters_actually_moving() {
    let words: Vec<String> = ["MAKE", "EVERY", "EDIT", "COUNT"]
        .iter()
        .enumerate()
        .map(|(i, w)| format!(r#"{{"id":"w{i}","type":"text","text":"{w}","split":"chars"}}"#))
        .collect();
    let intro = |gap: &str| -> Vec<String> {
        (0..4)
            .map(|i| {
                let at = if i == 0 {
                    "0".to_owned()
                } else {
                    format!(r#""<{gap}""#)
                };
                format!(
                    r##"{{"from":"#w{i}","at":{at},"blur":8,"opacity":0,"dur":0.4,"stagger":0.03}}"##
                )
            })
            .collect()
    };
    // 0.2 s apart, at most 10 letters blur at once (at 0.6 s: 5 of EVERY, 4 of EDIT, 1 of COUNT), not 14.
    let spaced = intro("0.2");
    assert!(
        checked(&scene(3.0, &words, &spaced)).is_ok(),
        "{:?}",
        checked(&scene(3.0, &words, &spaced))
    );
    assert!(at_once(3.0, &words, &spaced) <= 10);
    // 0.1 s apart, 17 do (at 0.4 s: 4, 5, 4 and 4), and the intro is refused at a tween.
    let close = intro("0.1");
    assert_eq!(at_once(3.0, &words, &close), 17);
    let e = checked(&scene(3.0, &words, &close)).expect_err("17 at once");
    assert!(
        e.iter()
            .any(|p| p.code == "scene.blurringUnits" && p.path.starts_with("/timeline/")),
        "{e:?}"
    );
}

#[test]
fn the_refusal_names_the_tween_whose_stagger_moves_too_many() {
    // The 900-filler repro: 94 letters at once. The tween's stagger and dur are what to change, not the first text.
    let nodes = [
        r#"{"type":"repeat","times":12,"children":[{"type":"text","class":"t","text":"ABCDEFGHIJKL","split":"chars"}]}"#,
        r#"{"type":"repeat","times":900,"children":[{"type":"text","class":"t","text":"x"}]}"#,
    ]
    .map(str::to_owned);
    let tween = one(r#"{"to":".t","blur":6,"opacity":0,"dur":0.09,"stagger":{"amount":1}}"#);
    let e = checked(&scene(2.0, &nodes, &tween)).expect_err("94 at once");
    assert!(
        e.iter()
            .any(|p| p.code == "scene.blurringUnits" && p.path == "/timeline/0"),
        "{e:?}"
    );
    // Past 13, the innermost layer carries the rest: the whole count is still named.
    assert_eq!(at_once(2.0, &nodes, &tween), 94);
}

/// SplitMix64: a seeded generator, so every run checks the same scenes.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn between(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

/// One tween's units: `members` of them, `each` apart from `start`, each moving for `dur`.
struct Run {
    start: f64,
    each: f64,
    dur: f64,
    members: u64,
}

/// Up to 8 tweens, each blurring in or out its own 1 to 3 split texts of 1 to 12 letters, as GSAP staggers do: a
/// number, `each` from the start, end or at random, or an `amount`. Times are whole 128ths of a second, so every sum
/// is exact. A tween moves for 1 to 12 times its stagger (one in 8 may leave gaps between its units).
fn random_scene(rng: &mut Rng) -> (Vec<String>, Vec<String>, Vec<Run>) {
    let (mut nodes, mut timeline, mut runs) = (Vec::new(), Vec::new(), Vec::new());
    let mut at = 0u64; // 64ths of a second
    for j in 0..rng.between(1, 8) {
        let count = if rng.between(0, 3) == 0 {
            rng.between(2, 3)
        } else {
            1
        };
        let mut members = 0;
        for n in 0..count {
            let len = rng.between(1, 12);
            members += len;
            nodes.push(format!(
                r#"{{"id":"n{j}x{n}","class":"c{j}","type":"text","text":"{}","split":"chars"}}"#,
                &LETTERS[..len as usize]
            ));
        }
        let each128 = rng.between(1, 8);
        let dur64 = if rng.between(0, 7) == 0 {
            rng.between(1, each128.div_ceil(2))
        } else {
            rng.between(each128.div_ceil(2), 6 * each128)
        };
        let (each, dur, start) = (
            each128 as f64 / 128.0,
            dur64 as f64 / 64.0,
            at as f64 / 64.0,
        );
        let stagger = match rng.between(0, 3) {
            0 => format!("{each}"),
            1 => {
                let from = ["start", "end", "random"][rng.between(0, 2) as usize];
                format!(r#"{{"each":{each},"from":"{from}"}}"#)
            }
            2 if members > 1 => format!(r#"{{"amount":{}}}"#, each * (members - 1) as f64),
            2 => format!("{each}"),
            _ => format!(r#"{{"each":{each}}}"#),
        };
        let verb = ["from", "to"][rng.between(0, 1) as usize];
        let selector = if count == 1 {
            format!("#n{j}x0")
        } else {
            format!(".c{j}")
        };
        timeline.push(format!(
            r#"{{"{verb}":"{selector}","at":{start},"blur":8,"opacity":0,"dur":{dur},"stagger":{stagger}}}"#
        ));
        runs.push(Run {
            start,
            each,
            dur,
            members,
        });
        at += rng.between(0, 24);
    }
    (nodes, timeline, runs)
}

/// The most units moving at one frame (256 a second, which every unit's start and end fall on), each counted from
/// its start to its end inclusive, as the checker's sweep counts.
fn simulate(runs: &[Run], duration: f64) -> usize {
    let windows: Vec<(f64, f64)> = runs
        .iter()
        .flat_map(|r| {
            (0..r.members).map(move |k| {
                let start = r.start + k as f64 * r.each;
                (start, start + r.dur)
            })
        })
        .collect();
    (0..=(duration * 256.0) as u32)
        .map(|frame| {
            let t = f64::from(frame) / 256.0;
            windows.iter().filter(|(a, b)| *a <= t && t <= *b).count()
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn the_count_bounds_a_frame_by_frame_simulation_within_twice() {
    let mut rng = Rng(0x5EED);
    for case in 0..200 {
        let (nodes, timeline, runs) = random_scene(&mut rng);
        let (model, real) = (at_once(6.0, &nodes, &timeline), simulate(&runs, 6.0));
        let json = scene(6.0, &nodes, &timeline);
        assert!(model >= real, "case {case}: {model} below {real}: {json}");
        // With gaps between a tween's units, its layers count it across them: the bound holds, but not within twice.
        if runs.iter().all(|r| r.dur >= r.each) {
            assert!(
                model <= 2 * real,
                "case {case}: {model} over twice {real}: {json}"
            );
        }
    }
}
