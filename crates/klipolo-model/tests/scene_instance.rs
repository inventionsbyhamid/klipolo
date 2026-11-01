//! A scene instance's renderer-facing side: static fields bound to a table row are held to
//! their fields' ranges, units can be set in one batch, and a frame counts and bounds its own work: track
//! reads and particle candidates.

use klipolo_model::scene::eval::{eval_into, SceneFrame};
use klipolo_model::scene::instance::{Field, SceneInstance};
use klipolo_model::scene::timeline::{compile, CompiledScene};
use klipolo_model::scene::{Points, Scene, SceneLimits, Val};
use std::sync::Arc;

fn scene(json: &str) -> Arc<CompiledScene> {
    Arc::new(compile(&Scene::from_stored(json.as_bytes()).unwrap()))
}

/// A `$row.vN` the check can't range (a table's values have no bounds) reaches the renderer held to its field's
/// range: a star's sides 3–64, a box edge within its bounds, a polyline's points within its box (a chart).
#[test]
fn row_bound_static_fields_are_held_to_their_ranges() {
    let s = scene(
        r##"{"scene":1,"duration":2,"params":[{"key":"t","label":"T","type":"table","columns":["a","b"],"maxRows":2,
          "default":[{"label":"x","values":[1e20,-1e20]}]}],
        "nodes":[{"type":"repeat","rows":"$t","children":[
          {"type":"star","id":"s","box":[0,0,0.1,0.1],"sides":"$row.v0","innerRadius":0.5},
          {"type":"polyline","id":"p","points":"$t","stroke":"#FFFFFF"}]}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let star = inst.instances_of(s.find("s").unwrap()).next().unwrap();
    let row = |v: &str| Val::Str(v.to_owned());
    assert_eq!(
        inst.number_of(star, Field::Sides, &row("$row.v0")),
        Some(64.0)
    );
    assert_eq!(
        inst.number_of(star, Field::Sides, &Val::Num(4.6)),
        Some(5.0)
    );
    assert_eq!(
        inst.number_of(star, Field::BoxW, &row("$row.v1")),
        Some(0.0)
    );
    assert_eq!(
        inst.number_of(star, Field::BoxX, &row("$row.v0")),
        Some(10.0)
    );
    // A table's rows draw as a chart: their values are spread over the box, so none reaches past it.
    let line = inst.instances_of(s.find("p").unwrap()).next().unwrap();
    assert_eq!(
        inst.points_of(line, &Points::Bind("$t".into())),
        vec![[0.5, 0.5]]
    );
}

/// Units set in one batch place the tweens as setting them one by one does.
#[test]
fn units_set_in_one_batch_place_tweens_as_one_by_one() {
    let s = scene(
        r##"{"scene":1,"duration":4,"nodes":[{"type":"text","id":"a","text":"one two three","split":"words","box":[0,0,1,1]},
          {"type":"text","id":"b","text":"four five","split":"words","box":[0,0,1,1]},{"type":"rect","id":"r","box":[0,0,0.1,0.1]}],
        "timeline":[{"from":"text","dur":0.5,"opacity":0,"stagger":0.1},{"to":"#r","at":">","dur":0.5,"x":0.5}]}"##,
    );
    let (mut one, mut batch) = (
        SceneInstance::new(&s, &Default::default()),
        SceneInstance::new(&s, &Default::default()),
    );
    one.set_units(0, 2);
    one.set_units(1, 1);
    batch.set_units_many(&[(0, 2), (1, 1)]);
    let (mut fa, mut fb) = (
        SceneFrame::for_instance(&one),
        SceneFrame::for_instance(&batch),
    );
    for t in [0.2, 0.6, 0.8, 1.0] {
        eval_into(&one, t, &mut fa);
        eval_into(&batch, t, &mut fb);
        assert_eq!(fa.nodes(), fb.nodes(), "at {t} s");
    }
}

/// An instance past the track cap is decided once, when it is built: every frame is its settled still, whole, reading
/// no tracks (the renderer shows it with one diagnostic). A common scene reads a few dozen tracks a frame.
#[test]
fn an_instance_past_the_track_cap_draws_its_settled_still() {
    let tweens: Vec<&str> = (0..256)
        .map(|_| r#"{"to":".c","at":0,"dur":1,"x":0.1,"y":0.1,"opacity":0.5,"scale":1.2,"rotate":10}"#)
        .collect();
    let s = scene(&format!(
        r#"{{"scene":1,"duration":4,"nodes":[{{"type":"repeat","times":1000,"children":[{{"type":"rect","class":"c","box":[0,0,0.01,0.01]}}]}}],
          "timeline":[{}]}}"#,
        tweens.join(",")
    ));
    let inst = SceneInstance::new(&s, &Default::default());
    assert!(
        inst.track_count() <= 256 * 8,
        "shared: {} tracks",
        inst.track_count()
    );
    assert!(inst.worst_reads() > SceneLimits::MAX_TRACKS);
    let (mut a, mut b) = (
        SceneFrame::for_instance(&inst),
        SceneFrame::for_instance(&inst),
    );
    eval_into(&inst, 0.2, &mut a);
    eval_into(&inst, 0.7, &mut b);
    let counts = a.counts();
    assert!(
        counts.still && counts.over().tracks && counts.track_evals == 0,
        "{counts:?}"
    );
    assert_eq!(a.nodes(), b.nodes(), "the same still on every frame");
    assert!(
        a.nodes()[1..]
            .iter()
            .all(|n| (n.x - 0.1).abs() < 1e-9 && (n.scale - 1.2).abs() < 1e-9),
        "whole"
    );

    let lower_third = Arc::new(compile(
        &Scene::from_stored(include_bytes!("fixtures/scenes/lower-third.json")).unwrap(),
    ));
    let inst = SceneInstance::new(&lower_third, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 0.5, &mut f);
    let counts = f.counts();
    assert!(
        counts.track_evals > 0 && counts.track_evals < 200,
        "{counts:?}"
    );
    assert!(counts.track_evals <= inst.worst_reads(), "{counts:?}");
    assert!(!counts.over().any(), "{counts:?}");
}

/// The check's count is an upper bound on what the instance reads, and equal to it for
/// repeated shapes: seven chained `x` tweens over 1,000 rects read 56,000 tracks at most, just under the cap.
#[test]
fn the_check_and_the_instance_count_the_same_reads() {
    let list: Vec<String> = (0..7)
        .map(|k| {
            format!(
                r#"{{"to":".c","at":0,"dur":1,"x":{}}}"#,
                0.1 * (k + 1) as f64
            )
        })
        .collect();
    let json = format!(
        r#"{{"scene":1,"duration":2,"nodes":[{{"type":"repeat","times":1000,"children":[{{"type":"rect","class":"c","box":[0,0,0.01,0.01]}}]}}],
          "timeline":[{}]}}"#,
        list.join(",")
    );
    let scene = Scene::from_stored(json.as_bytes()).unwrap();
    let facts =
        klipolo_model::scene::check_scene(&scene, klipolo_model::contrib::Catalog::builtin())
            .unwrap();
    let inst = SceneInstance::new(&Arc::new(compile(&scene)), &Default::default());
    assert_eq!(facts.track_reads, 56_000);
    assert_eq!(inst.worst_reads(), facts.track_reads);
    for name in [
        "lower-third.json",
        "kinetic.json",
        "kitchen-sink.json",
        "kitchen-sink-2.json",
    ] {
        let path = format!(
            "{}/tests/fixtures/scenes/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let scene = Scene::from_stored(&std::fs::read(path).unwrap()).unwrap();
        let facts =
            klipolo_model::scene::check_scene(&scene, klipolo_model::contrib::Catalog::builtin())
                .unwrap();
        let inst = SceneInstance::new(&Arc::new(compile(&scene)), &Default::default());
        assert!(
            inst.worst_reads() <= facts.track_reads,
            "{name}: {} > {}",
            inst.worst_reads(),
            facts.track_reads
        );
    }
}

/// A frame looks at no more than four particle candidates per particle it may write, over every emitter together.
#[test]
fn a_frame_looks_at_a_bounded_number_of_particles() {
    let s = scene(
        r##"{"scene":1,"duration":4,"nodes":[{"type":"repeat","times":100,"children":[
          {"type":"emitter","box":[0,0,1,1],"burst":500,"life":2,"child":{"type":"ellipse","box":[0,0,0.01,0.01]}}]}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 0.5, &mut f);
    let counts = f.counts();
    assert_eq!(f.particles().len(), SceneLimits::MAX_PARTICLES);
    assert!(
        counts.particle_looks <= 4 * SceneLimits::MAX_PARTICLES,
        "{counts:?}"
    );
    assert!(counts.dropped_particles > 0 && counts.over().particles);
}

/// A `set` or a 0 s tween placed at the same moment as a timed tween wins by list order, as
/// any later tween does: both are ordered by where the timeline placed them, not by their first key (a microsecond
/// earlier for a change at one instant).
#[test]
fn a_set_or_an_instant_tween_at_the_same_moment_wins_by_list_order() {
    let at = |timeline: &str, t: f64| {
        let s = scene(&format!(
            r##"{{"scene":1,"duration":3,"nodes":[{{"type":"rect","id":"a","box":[0,0,0.1,0.1]}}],"timeline":[{timeline}]}}"##
        ));
        let inst = SceneInstance::new(&s, &Default::default());
        let mut f = SceneFrame::for_instance(&inst);
        eval_into(&inst, t, &mut f);
        f.node("a").x
    };
    let timed = r##"{"to":"#a","x":1,"at":1,"dur":1,"ease":"none"}"##;
    for later in [
        r##"{"set":"#a","x":0.5,"at":1}"##,
        r##"{"to":"#a","x":0.5,"at":1,"dur":0}"##,
    ] {
        let listed_after = format!("{timed},{later}");
        for t in [1.0, 1.5, 2.5] {
            assert!(
                (at(&listed_after, t) - 0.5).abs() < 1e-9,
                "{later} listed after the timed tween, at {t} s"
            );
        }
        let listed_before = format!("{later},{timed}");
        assert!(
            (at(&listed_before, 1.5) - 0.75).abs() < 1e-9,
            "{later} listed first: the timed tween eases on from 0.5"
        );
        assert!((at(&listed_before, 2.5) - 1.0).abs() < 1e-9);
    }
}

/// The hold point: an instance past the track cap draws its still at the latest moment
/// before an exit starts, so a scene ending in a fade-out shows every rect where it rests, not a blank frame.
#[test]
fn a_still_past_the_track_cap_is_drawn_before_the_exit() {
    let mut list: Vec<String> = (0..8)
        .map(|k| {
            format!(
                r#"{{"to":".c","at":0,"dur":1,"x":{}}}"#,
                0.1 * (k + 1) as f64
            )
        })
        .collect();
    list.push(r#"{"to":".c","at":3,"dur":1,"opacity":0}"#.to_owned());
    let s = scene(&format!(
        r#"{{"scene":1,"duration":4,"nodes":[{{"type":"repeat","times":1000,"children":[{{"type":"rect","class":"c","box":[0,0,0.01,0.01]}}]}}],
          "timeline":[{}]}}"#,
        list.join(",")
    ));
    let inst = SceneInstance::new(&s, &Default::default());
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, 3.9, &mut f);
    assert!(f.counts().still, "{:?}", f.counts());
    for n in [1, 500, 1000] {
        let rect = f.nodes()[n];
        assert!(rect.drawn && rect.opacity == 1.0, "rect {n}: {rect:?}");
        assert!(
            (rect.x - 0.8).abs() < 1e-9,
            "rect {n} rests at 0.8: {}",
            rect.x
        );
    }
}
