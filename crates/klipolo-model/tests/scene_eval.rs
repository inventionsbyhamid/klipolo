//! The scene timeline compiler and evaluator: tweens land where GSAP places them, segments stretch the
//! hold and keep the entrance and exit, staggers spread over the units actually laid out, repeats and
//! yoyos cycle per unit, values stay in their fields' ranges, settings and table rows bind, and a still hold says how
//! long nothing changes.

use klipolo_model::params::ParamValue;
use klipolo_model::scene::{
    eval::*, instance::SceneInstance, segments::scene_time, timeline::compile, Scene, Segments,
};
use klipolo_model::timeline::PieceTime;
use std::collections::BTreeMap;
use std::sync::Arc;

fn scene(json: &str) -> Arc<klipolo_model::scene::timeline::CompiledScene> {
    Arc::new(compile(&Scene::from_stored(json.as_bytes()).unwrap()))
}

#[test]
fn tweens_land_where_gsap_places_them() {
    let s = scene(
        r##"{"scene":1,"duration":3,"nodes":[{"type":"rect","id":"a","box":[0,0,0.1,0.1]},{"type":"rect","id":"b","box":[0,0,0.1,0.1]}],
      "timeline":[{"from":"#a","dur":0.5,"x":-0.2},{"to":"#b","dur":0.5,"x":0.3},{"to":"#a","at":"<","dur":0.5,"opacity":0},
                  {"to":"#b","at":"+=0.25","dur":0.25,"scale":2}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 0.0, &mut f);
    assert!((f.node("a").x - -0.2).abs() < 1e-9); // from: starts at the given value
    eval_into(&inst, 0.5, &mut f);
    assert!(f.node("a").x.abs() < 1e-9 && f.node("b").x.abs() < 1e-9);
    eval_into(&inst, 1.0, &mut f);
    assert!((f.node("b").x - 0.3).abs() < 1e-9);
    assert!(f.node("a").opacity.abs() < 1e-9); // "<": with the tween before it, 0.5–1.0
    eval_into(&inst, 1.25, &mut f);
    assert!((f.node("b").scale - 1.0).abs() < 1e-9); // "+=0.25": starts at 1.25
    eval_into(&inst, 1.5, &mut f);
    assert!((f.node("b").scale - 2.0).abs() < 1e-9);
}

#[test]
fn segments_stretch_the_hold_and_keep_in_and_out() {
    let seg = Some(Segments {
        enter: 1.0,
        out: 0.5,
    });
    let us = |s: f64| (s * 1e6) as i64;
    // A 4 s scene on a 12 s layer: in plays as written, out ends with the layer, the 2.5 s hold fills 10.5 s.
    assert_eq!(scene_time(us(12.0), us(0.5), 4.0, seg), 0.5);
    assert!((scene_time(us(12.0), us(11.75), 4.0, seg) - 3.75).abs() < 1e-9);
    assert!((scene_time(us(12.0), us(6.25), 4.0, seg) - 2.25).abs() < 1e-9);
    // On a 1 s layer both squeeze to fit: at the end it is the scene's end.
    assert!((scene_time(us(1.0), us(1.0), 4.0, seg) - 4.0).abs() < 1e-9);
    assert!(scene_time(us(1.0), us(0.3), 4.0, seg) < 1.0);
    // Without segments the scene plays at its own speed and holds its last frame.
    assert_eq!(scene_time(us(10.0), us(7.0), 4.0, None), 4.0);

    // A 6 s layer split at 2 s. Both pieces keep the 6 s span; the second starts 2 s into it, so it
    // continues the hold where the first left it and doesn't replay the entrance; its exit ends with it.
    use klipolo_model::scene::segments::piece_scene_time as at;
    let first = PieceTime {
        offset_us: 0,
        span_us: us(6.0),
    };
    let second = PieceTime {
        offset_us: us(2.0),
        span_us: us(6.0),
    };
    let split_point = at(first, us(2.0), 4.0, seg);
    assert!((at(second, 0, 4.0, seg) - split_point).abs() < 1e-9);
    assert!(at(second, 0, 4.0, seg) > 1.0, "the entrance isn't replayed");
    assert!(
        (at(second, us(4.0), 4.0, seg) - 4.0).abs() < 1e-9,
        "the exit ends with the second piece"
    );
    // Trimming the second piece's end by 1 s (its span shrinks with it): its exit still ends with it.
    let trimmed = PieceTime {
        offset_us: us(2.0),
        span_us: us(5.0),
    };
    assert!((at(trimmed, us(3.0), 4.0, seg) - 4.0).abs() < 1e-9);
    // Lengthening the first piece by 1 s grows its span to 7 s and not the second's (accepted behaviour):
    // the moving hold now jumps back a little at the split point, and the entrance stays at the start.
    let longer = PieceTime {
        offset_us: 0,
        span_us: us(7.0),
    };
    let end_of_first = at(longer, us(3.0), 4.0, seg);
    let jump = end_of_first - at(second, 0, 4.0, seg);
    assert!(jump > 0.0 && jump < 0.5, "a small jump back: {jump}");
    assert_eq!(at(longer, us(0.5), 4.0, seg), 0.5);
    assert_eq!(klipolo_model::scene::segments::settled_time(4.0, seg), 1.0);
}

#[test]
fn stagger_orders_units_and_repeat_yoyo_cycles() {
    let s = scene(
        r##"{"scene":1,"duration":4,"nodes":[{"type":"text","id":"t","text":"one two three","split":"words","box":[0,0,1,1]}],
      "timeline":[{"from":"#t","dur":0.4,"y":0.1,"opacity":0,"stagger":{"each":0.1,"from":"end"}},
                  {"to":"#t","at":1,"dur":0.5,"rotate":10,"repeat":-1,"yoyo":true}]}"##,
    );
    let mut inst = SceneInstance::new(&s, &Default::default());
    inst.set_units_by_id("t", 3);
    let mut f = SceneFrame::for_instance(&inst);
    // A tween with `stagger` on a split text node targets its units (one without targets the node). `from: "end"`
    // starts unit 2 at 0, unit 1 at 0.1 and unit 0 at 0.2; at 0.15 unit 2 is 37.5 % in, unit 1 12.5 %, unit 0 not
    // started (it holds the `from` value, opacity 0).
    eval_into(&inst, 0.15, &mut f);
    let u = f.units_of("t");
    assert!(
        u[2].opacity > u[1].opacity && u[1].opacity > u[0].opacity,
        "from: end goes first"
    );
    assert_eq!(u[0].opacity, 0.0);
    eval_into(&inst, 1.5, &mut f);
    assert!((f.node("t").rotate - 10.0).abs() < 1e-9);
    eval_into(&inst, 2.0, &mut f);
    assert!(f.node("t").rotate.abs() < 1e-9, "yoyo comes back");
}

#[test]
fn value_animators_and_modifiers_are_deterministic() {
    assert_eq!(
        klipolo_model::scene::modifiers::count_text(10_000.0, "#,##0", "", " subscribers").as_str(),
        "10,000 subscribers"
    );
    let a = klipolo_model::scene::modifiers::wiggle(1.234, 3.0, 0.1, 7);
    assert_eq!(
        a,
        klipolo_model::scene::modifiers::wiggle(1.234, 3.0, 0.1, 7)
    );
    assert_ne!(
        a,
        klipolo_model::scene::modifiers::wiggle(1.234, 3.0, 0.1, 8)
    );
}

#[test]
fn a_still_hold_reports_how_long_nothing_changes() {
    let s = scene(
        r##"{"scene":1,"duration":4,"segments":{"in":1,"out":1},"nodes":[{"type":"rect","id":"a","box":[0,0,1,1]}],
      "timeline":[{"from":"#a","dur":1,"opacity":0},{"to":"#a","at":"out","dur":1,"opacity":0}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    assert_eq!(klipolo_model::scene::eval::static_until(&inst, 1.5), 3.0);
}

#[test]
fn a_backdrop_an_animating_fill_or_live_particles_are_never_static() {
    // The same still hold, plus each input that changes on its own: `static_until` returns `t` itself.
    let with = |extra: &str| {
        scene(&format!(
            r##"{{"scene":1,"duration":4,"segments":{{"in":1,"out":1}},"nodes":[{{"type":"rect","id":"a","box":[0,0,1,1]}}{extra}],
      "timeline":[{{"from":"#a","dur":1,"opacity":0}},{{"to":"#a","at":"out","dur":1,"opacity":0}}]}}"##
        ))
    };
    let backdrop = SceneInstance::new(
        &with(r#",{"type":"backdrop","box":[0,0,1,1],"blur":8}"#),
        &Default::default(),
    );
    assert_eq!(
        klipolo_model::scene::eval::static_until(&backdrop, 1.5),
        1.5
    );
    let particles = SceneInstance::new(
        &with(
            r#",{"type":"emitter","box":[0,0,1,1],"rate":20,"life":[1,2],"child":{"type":"ellipse","box":[0,0,0.01,0.01]}}"#,
        ),
        &Default::default(),
    );
    assert_eq!(
        klipolo_model::scene::eval::static_until(&particles, 1.5),
        1.5
    );
    let mut fill = SceneInstance::new(&with(""), &Default::default());
    fill.set_fills_animate(true); // the renderer sets it when a generator fill's shader animates
    assert_eq!(klipolo_model::scene::eval::static_until(&fill, 1.5), 1.5);
}

/// A stagger spreads over the units actually laid out (at most what the check counted, a bound
/// text at its maxLen), and the tweens after it start where it really ends.
#[test]
fn later_tweens_start_where_a_stagger_over_the_laid_out_units_ends() {
    let s = scene(
        r##"{"scene":1,"duration":4,"params":[{"key":"word","label":"Word","type":"text","default":"LAUNCH","maxLen":24}],
      "nodes":[{"type":"text","id":"w","text":"$word","split":"chars","box":[0,0,1,1]},{"type":"rect","id":"r","box":[0,0,0.1,0.1]}],
      "timeline":[{"from":"#w","dur":0.5,"opacity":0,"stagger":0.1},{"to":"#r","at":">","dur":0.5,"x":0.5,"ease":"none"}]}"##,
    );
    let x_at = |inst: &SceneInstance, t: f64| {
        let mut f = SceneFrame::for_instance(inst);
        eval_into(inst, t, &mut f);
        f.node("r").x
    };
    // "LAUNCH": 6 units, the stagger spreads 0.5 s, so the rect moves from 1.0 s.
    let mut inst = SceneInstance::new(&s, &Default::default());
    assert_eq!(inst.units(0), 6);
    assert!(x_at(&inst, 1.0).abs() < 1e-9 && (x_at(&inst, 1.25) - 0.25).abs() < 1e-9);
    // Laid out in 3 units: it ends at 0.7 s.
    inst.set_units_by_id("w", 3);
    assert!((x_at(&inst, 0.95) - 0.25).abs() < 1e-9);
    // More units than the check counted are held to it (24, the setting's maxLen): it ends at 2.8 s.
    inst.set_units_by_id("w", 100);
    assert_eq!(inst.units(0), 24);
    assert!(x_at(&inst, 2.8).abs() < 1e-9 && (x_at(&inst, 3.3) - 0.5).abs() < 1e-9);
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 0.0, &mut f);
    assert_eq!(f.units_of("w").len(), 24);
}

/// Every evaluated value stays in its field's range after modifiers, with bound settings held to their specs and a
/// table row's counter within ±1e15; a bound `times` draws at its ceiling.
#[test]
fn values_stay_in_range_after_modifiers_and_bindings() {
    let s = scene(
        r##"{"scene":1,"duration":4,"params":[{"key":"o","label":"O","type":"float","min":0,"max":1,"default":0.5},
        {"key":"n","label":"N","type":"float","min":0,"max":10,"default":2.2},
        {"key":"data","label":"Data","type":"table","columns":["n"],"maxRows":3,"default":[{"label":"a","values":[1e20]}]}],
      "nodes":[{"type":"rect","id":"a","box":[0,0,1,1],"opacity":"$o","modifiers":{"opacity":{"type":"oscillate","freq":1,"amp":1}}},
        {"type":"repeat","times":"$n","children":[{"type":"ellipse","id":"dot","box":[0,0,0.05,0.05],"x":"$i"}]},
        {"type":"repeat","rows":"$data","children":[{"type":"text","id":"c","text":"0","count":{"from":0,"to":"$row.v0","at":0,"dur":1}}]}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 0.25, &mut f);
    assert_eq!(f.node("a").opacity, 1.0, "0.5 + 1 is held at 1");
    eval_into(&inst, 0.75, &mut f);
    assert_eq!(f.node("a").opacity, 0.0, "0.5 − 1 is held at 0");
    let dots: Vec<usize> = inst.instances_of(s.find("dot").unwrap()).collect();
    assert_eq!(dots.len(), 3, "2.2 instances draw as 3");
    assert_eq!(f.nodes()[dots[2]].x, 2.0);
    let c = inst.instances_of(s.find("c").unwrap()).next().unwrap();
    eval_into(&inst, 1.0, &mut f);
    assert_eq!(f.text(&inst, c), "1000000000000000");
    // A layer's value past the setting's maximum is held to it.
    let loud = SceneInstance::new(
        &s,
        &BTreeMap::from([("o".to_owned(), ParamValue::Number(5.0))]),
    );
    let mut f = SceneFrame::for_instance(&loud);
    eval_into(&loud, 0.0, &mut f);
    assert_eq!(f.node("a").opacity, 1.0);
}

/// Yoyo with an odd repeat ends on the start values; `random` is a shuffle of the whole stagger steps.
#[test]
fn an_odd_yoyo_ends_on_its_start_and_random_shuffles_whole_steps() {
    let s = scene(
        r##"{"scene":1,"duration":9,"nodes":[{"type":"rect","id":"a","box":[0,0,0.1,0.1]},
        {"type":"text","id":"t","text":"abcdefgh","split":"chars","box":[0,0,1,1]}],
      "timeline":[{"to":"#a","at":0,"dur":1,"x":0.5,"repeat":1,"yoyo":true},
        {"from":"#t","at":0,"dur":0.1,"opacity":0,"stagger":{"each":1,"from":"random","seed":3}}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 1.0, &mut f);
    assert!((f.node("a").x - 0.5).abs() < 1e-9);
    for t in [2.0, 5.0] {
        eval_into(&inst, t, &mut f);
        assert_eq!(f.node("a").x, 0.0, "at {t} s it holds its start");
    }
    let mut order = Vec::new();
    for k in 0..8 {
        eval_into(&inst, k as f64 + 0.05, &mut f);
        let started: Vec<usize> = (0..8)
            .filter(|&i| f.units_of("t")[i].opacity > 0.0)
            .collect();
        assert_eq!(started.len(), k + 1, "one more unit starts each second");
        let new: Vec<usize> = started.into_iter().filter(|i| !order.contains(i)).collect();
        order.extend(new);
    }
    assert_ne!(
        order,
        (0..8).collect::<Vec<_>>(),
        "random isn't document order"
    );
}

/// A text key or a tween's text replaces the text (before its first key the node shows its own); a number shows in at
/// most 24 characters.
#[test]
fn text_keys_and_tween_texts_replace_the_text() {
    let s = scene(
        r##"{"scene":1,"duration":3,"nodes":[
        {"type":"text","id":"t","text":"SUBSCRIBE","box":[0,0,1,1],"animate":{"text":[{"t":1,"v":"SUBSCRIBED"},{"t":2,"v":1e300}]}},
        {"type":"text","id":"u","text":"Loading","box":[0,0,1,1]}],
      "timeline":[{"to":"#u","at":1,"dur":0.5,"text":"Done"}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    let (t, u) = (0, 1);
    for (at, shown, other) in [
        (0.5, "SUBSCRIBE", "Loading"),
        (1.2, "SUBSCRIBED", "Loading"),
        (2.5, "1e300", "Done"),
    ] {
        eval_into(&inst, at, &mut f);
        assert_eq!(
            (f.text(&inst, t), f.text(&inst, u)),
            (shown, other),
            "at {at} s"
        );
    }
    assert!(f.counts().chars <= 24 + 4);
}

/// Repeats expand per instance and per table row, with `$i`, `$row.label` and `$row.v0`, moved by `gap`.
#[test]
fn repeats_expand_rows_with_their_values_and_gaps() {
    let s = scene(
        r##"{"scene":1,"duration":2,"params":[{"key":"bars","label":"Bars","type":"table","columns":["v"],"maxRows":5,
          "default":[{"label":"A","values":[0.2]},{"label":"B","values":[0.6]}]}],
      "nodes":[{"type":"repeat","rows":"$bars","gap":[0.25,0],"children":[
        {"type":"rect","id":"bar","box":[0,0,0.1,1],"animate":{"scaleY":[{"t":0,"v":0},{"t":1,"v":"$row.v0"}]}},
        {"type":"text","id":"lbl","text":"$row.label","box":[0,0,0.1,0.1]}]}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 1.0, &mut f);
    let bars: Vec<usize> = inst.instances_of(s.find("bar").unwrap()).collect();
    let labels: Vec<usize> = inst.instances_of(s.find("lbl").unwrap()).collect();
    assert_eq!(bars.len(), 2);
    assert!(
        (f.nodes()[bars[0]].scale_y - 0.2).abs() < 1e-9
            && (f.nodes()[bars[1]].scale_y - 0.6).abs() < 1e-9
    );
    assert_eq!((f.nodes()[bars[0]].x, f.nodes()[bars[1]].x), (0.0, 0.25));
    assert_eq!(
        (f.text(&inst, labels[0]), f.text(&inst, labels[1])),
        ("A", "B")
    );
}

/// Particles are seeded (the same frame twice is the same), die after their life, and a frame writes at most 500.
#[test]
fn particles_are_seeded_and_bounded() {
    let s = scene(
        r##"{"scene":1,"duration":4,"nodes":[{"type":"emitter","id":"e","box":[0,0,1,1],"burst":50,"life":1,"seed":9,
        "gravity":100,"child":{"type":"ellipse","box":[0,0,0.01,0.01]}}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let (mut a, mut b) = (
        SceneFrame::for_instance(&inst),
        SceneFrame::for_instance(&inst),
    );
    eval_into(&inst, 0.5, &mut a);
    eval_into(&inst, 0.5, &mut b);
    assert_eq!(a.particles().len(), 50);
    assert_eq!(a.particles(), b.particles());
    assert!(a.particles().iter().all(|p| (p.age - 0.5).abs() < 1e-9));
    eval_into(&inst, 1.5, &mut a);
    assert!(a.particles().is_empty());
    assert_eq!(
        klipolo_model::scene::eval::static_until(&inst, 1.5),
        f64::INFINITY
    );
}

/// A lower third: the card is clipped from its first frame (a `set` shows from its own time), the clip
/// wipes open between lengths written in different units, `.line` staggers over the name's laid-out words and then the
/// role, and the exit runs at `out`; the hold between is still.
#[test]
fn the_lower_third_wipes_in_staggers_its_lines_and_exits_at_out() {
    let s = Arc::new(compile(
        &Scene::from_stored(include_bytes!("fixtures/scenes/lower-third.json")).unwrap(),
    ));
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    let right = |f: &SceneFrame| {
        let clip = f.node("card").clip;
        assert_eq!(clip.shape, ClipShape::Inset);
        clip.v[1]
    };
    eval_into(&inst, 0.0, &mut f);
    assert_eq!(
        right(&f),
        [1.0, 0.0],
        "inset(0 100% 0 0) from the first frame"
    );
    eval_into(&inst, 0.375, &mut f);
    assert!(right(&f)[0] > 0.0 && right(&f)[0] < 1.0);
    eval_into(&inst, 0.65, &mut f);
    assert_eq!(right(&f), [0.0, 0.0]);
    // "Your name" is 2 words: they start at 0.34 and 0.44, and the role, the third member, at 0.54.
    eval_into(&inst, 0.5, &mut f);
    let words = f.units_of("name");
    assert_eq!(words.len(), 2);
    assert!(words[0].opacity > words[1].opacity && words[1].opacity > 0.0);
    assert_eq!((f.node("role").opacity, f.node("role").y_px), (0.0, 22.0));
    eval_into(&inst, 3.75, &mut f);
    assert_eq!((f.node("card").opacity, f.node("card").y_px), (0.0, 18.0));
    assert_eq!(static_until(&inst, 2.0), 3.4);
}

/// A counter's number takes at most 24 characters, as the check counts it, however wide it is.
#[test]
fn a_counters_number_takes_at_most_24_characters() {
    let wide =
        klipolo_model::scene::modifiers::count_text(-999_999_999_999_999.0, "#,##0.000000", "", "");
    assert!(wide.chars().count() <= 24, "{wide}");
    assert_eq!(
        wide.replace(',', "").parse::<f64>().unwrap(),
        -999_999_999_999_999.0
    );
    assert_eq!(
        klipolo_model::scene::modifiers::count_text(1234.5, "#,##0.0", "", "").as_str(),
        "1,234.5"
    );
}

/// A later staggered tween starts each unit from that unit's own state, read with the stagger
/// offsets both tweens give it.
#[test]
fn a_later_staggered_tween_starts_each_unit_from_its_own_state() {
    let s = scene(
        r##"{"scene":1,"duration":6,"nodes":[{"type":"text","id":"t","text":"abcd","split":"chars","box":[0,0,1,1]}],
      "timeline":[{"from":"#t","at":0,"dur":1,"opacity":0,"stagger":{"each":0.5}},
                  {"to":"#t","at":1,"dur":1,"opacity":0,"stagger":{"each":0.5,"from":"end"}}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 1.25, &mut f);
    // Unit 3 starts its exit at 1.0, before its entrance (1.5): it leaves from where it was, invisible.
    assert_eq!(f.units_of("t")[3].opacity, 0.0);
    // Unit 0 entered fully by 1.0 and exits from 2.5: it is still at 1.
    assert!((f.units_of("t")[0].opacity - 1.0).abs() < 1e-9);
}

/// Particles and eased clips stay in their fields' ranges.
#[test]
fn particles_and_eased_clips_stay_in_range() {
    let s = scene(
        r##"{"scene":1,"duration":60,"nodes":[
        {"type":"emitter","id":"e","box":[0,0,1,1],"burst":1,"life":60,"speed":0,"gravity":100000,
         "child":{"type":"ellipse","box":[0,0,0.01,0.01]}},
        {"type":"rect","id":"r","box":[0,0,1,1],"clip":"inset(0 1000% 0 0)"}],
      "timeline":[{"to":"#r","at":0,"dur":1,"ease":"elastic.out","clip":"inset(0 -1000% 0 0)"}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 59.0, &mut f);
    assert!(
        f.particles()[0].dy_px <= klipolo_model::scene::units::MAX_PX,
        "{}",
        f.particles()[0].dy_px
    );
    let mut most: f64 = 0.0;
    for i in 1..100 {
        eval_into(&inst, f64::from(i) / 100.0, &mut f);
        most = most.max(-f.node("r").clip.v[1][0]);
    }
    assert!(
        most > 9.0 && most <= klipolo_model::scene::units::MAX_FRACTION,
        "the overshoot is held: {most}"
    );
}

/// Overlapping tweens on one property resolve in timeline order, as GSAP renders its children: the
/// tween placed later wins once it has started for a member, whatever order the timeline lists them in.
#[test]
fn overlapping_staggers_resolve_in_timeline_order() {
    let make = |first: &str, second: &str| {
        scene(&format!(
            r##"{{"scene":1,"duration":4,"nodes":[{{"type":"text","id":"t","text":"abcd","split":"chars","box":[0,0,1,1]}}],
              "timeline":[{first},{second}]}}"##
        ))
    };
    let up = r##"{"to":"#t","at":0,"y":"40px","stagger":{"each":0.5}}"##;
    let down = r##"{"to":"#t","at":1,"y":"-40px","stagger":{"each":0.5,"from":"end"}}"##;
    let (listed, reversed) = (make(up, down), make(down, up));
    let (a, b) = (
        SceneInstance::new(&listed, &Default::default()),
        SceneInstance::new(&reversed, &Default::default()),
    );
    let (mut fa, mut fb) = (SceneFrame::for_instance(&a), SceneFrame::for_instance(&b));
    for t in [1.25, 1.5, 1.75, 2.0, 3.0] {
        eval_into(&a, t, &mut fa);
        eval_into(&b, t, &mut fb);
        assert_eq!(
            fa.units_of("t"),
            fb.units_of("t"),
            "the order listed doesn't matter at {t} s"
        );
    }
    // Unit 3 moves down from 1.0 (the later tween) and stays down: the earlier tween starting for it at 1.5 doesn't
    // take it back up.
    eval_into(&a, 2.0, &mut fa);
    assert_eq!(fa.units_of("t")[3].y_px, -40.0);
}

/// In the staggered scene above the last letter, hidden by the exit before its entrance began, stays hidden.
#[test]
fn an_exit_placed_later_keeps_the_last_letter_hidden() {
    let s = scene(
        r##"{"scene":1,"duration":6,"nodes":[{"type":"text","id":"t","text":"abcd","split":"chars","box":[0,0,1,1]}],
      "timeline":[{"from":"#t","at":0,"dur":1,"opacity":0,"stagger":{"each":0.5}},
                  {"to":"#t","at":1,"dur":1,"opacity":0,"stagger":{"each":0.5,"from":"end"}}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    for t in [2.0, 3.0, 5.0] {
        eval_into(&inst, t, &mut f);
        assert_eq!(f.units_of("t")[3].opacity, 0.0, "at {t} s");
    }
}

/// A split text's own random stagger, left unseeded, shuffles each repeat instance its own way.
#[test]
fn an_unseeded_random_stagger_shuffles_each_instance_its_own_way() {
    let s = scene(
        r##"{"scene":1,"duration":9,"nodes":[{"type":"repeat","times":2,"children":[
        {"type":"text","id":"t","text":"abcdefgh","split":"chars","box":[0,0,1,1],"stagger":{"each":1,"from":"random"},
         "animate":{"opacity":[{"t":0,"v":0},{"t":0.1,"v":1}]}}]}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let texts: Vec<usize> = inst.instances_of(s.find("t").unwrap()).collect();
    let mut f = SceneFrame::for_instance(&inst);
    let mut orders = [Vec::new(), Vec::new()];
    for k in 0..8 {
        eval_into(&inst, f64::from(k) + 0.05, &mut f);
        for (order, &node) in orders.iter_mut().zip(&texts) {
            let started: Vec<usize> = (0..8).filter(|&i| f.units(node)[i].opacity > 0.0).collect();
            let new: Vec<usize> = started.into_iter().filter(|i| !order.contains(i)).collect();
            order.extend(new);
        }
    }
    assert_eq!(orders[0].len(), 8);
    assert_ne!(orders[0], orders[1]);
}

/// A tween's clip is made once whatever it targets, so 256 clip tweens over 200 nodes keep every
/// node's clip (each node used to push its own copies, past the clips' 65,535 limit).
#[test]
fn clip_tweens_over_many_nodes_keep_every_nodes_clip() {
    let nodes: Vec<String> = (0..200)
        .map(|_| r#"{"type":"rect","class":"c","box":[0,0,0.1,0.1]}"#.to_owned())
        .collect();
    let tweens: Vec<&str> = (0..256)
        .map(|_| r#"{"fromTo":".c","at":0,"dur":1,"ease":"none","start":{"clip":"inset(0 100% 0 0)"},"clip":"inset(0 0% 0 0)"}"#)
        .collect();
    let s = scene(&format!(
        r#"{{"scene":1,"duration":2,"nodes":[{}],"timeline":[{}]}}"#,
        nodes.join(","),
        tweens.join(",")
    ));
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 0.5, &mut f);
    for node in [0, 199] {
        let clip = f.nodes()[node].clip;
        assert_eq!(clip.shape, ClipShape::Inset, "node {node}");
        assert!(
            (clip.v[1][0] - 0.5).abs() < 1e-9,
            "node {node}: {:?}",
            clip.v[1]
        );
    }
}

/// A frame is never torn. A scene past the track cap draws its settled still, whole and the same
/// on every frame (the renderer shows it with one diagnostic); one just under it animates every node on every frame.
#[test]
fn a_scene_near_or_past_the_track_cap_is_never_torn() {
    let make = |tweens: usize| {
        let list: Vec<String> = (0..tweens)
            .map(|k| {
                format!(
                    r#"{{"to":".c","at":0,"dur":1,"ease":"none","x":{}}}"#,
                    0.1 * (k + 1) as f64
                )
            })
            .collect();
        scene(&format!(
            r#"{{"scene":1,"duration":2,"nodes":[{{"type":"repeat","times":1000,"children":[{{"type":"rect","class":"c","box":[0,0,0.01,0.01]}}]}}],
              "timeline":[{}]}}"#,
            list.join(",")
        ))
    };
    for tweens in [7, 8] {
        let s = make(tweens);
        let inst = SceneInstance::new(&s, &Default::default());
        let mut f = SceneFrame::for_instance(&inst);
        let mut seen = Vec::new();
        for t in [0.25, 0.5, 0.75] {
            eval_into(&inst, t, &mut f);
            let (first, last) = (f.nodes()[1].x, f.nodes()[1000].x);
            assert_eq!(first, last, "{tweens} tweens at {t} s: every rect alike");
            assert!(first != 0.0, "{tweens} tweens at {t} s: not at rest");
            seen.push(first);
        }
        let moving = seen.windows(2).any(|w| w[0] != w[1]);
        assert_eq!(moving, tweens == 7, "{tweens} tweens: {seen:?}");
    }
}

/// The check counts what a frame can read (each component, chained reads), so 9 fill and stroke
/// tweens over 1,000 rects, which a frame reads about 72,000 times when settled, are refused.
#[test]
fn the_check_counts_what_a_frame_reads() {
    let tweens: Vec<&str> = (0..9)
        .map(|_| r##"{"to":".c","at":0,"dur":1,"fill":"#FF0000","stroke":"#00FF00"}"##)
        .collect();
    let json = format!(
        r#"{{"scene":1,"duration":2,"nodes":[{{"type":"repeat","times":1000,"children":[{{"type":"rect","class":"c","box":[0,0,0.01,0.01]}}]}}],
          "timeline":[{}]}}"#,
        tweens.join(",")
    );
    let scene = Scene::from_stored(json.as_bytes()).unwrap();
    let problems =
        klipolo_model::scene::check_scene(&scene, klipolo_model::contrib::Catalog::builtin())
            .unwrap_err();
    assert!(
        problems.iter().any(|p| p.code == "scene.tooManyTracks"),
        "{problems:?}"
    );
}
