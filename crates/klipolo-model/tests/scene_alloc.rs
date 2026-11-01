//! Its own binary, with a per-thread counting allocator: once warm, evaluating a scene's frame allocates nothing, and a
//! scene past every runtime cap evaluates within its caps, allocation-free, counting what it is past for the renderer's
//! diagnostics.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        BYTES.with(|c| c.set(c.get() + l.size() as u64));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static A: Counting = Counting;

#[test]
fn eval_allocates_nothing_once_warm() {
    let s = std::sync::Arc::new(klipolo_model::scene::timeline::compile(
        &klipolo_model::scene::Scene::from_stored(include_bytes!("fixtures/scenes/kinetic.json"))
            .unwrap(),
    ));
    let mut inst = klipolo_model::scene::instance::SceneInstance::new(&s, &Default::default());
    inst.set_units_by_id("word", 12); // the fixture's split title
    let mut f = klipolo_model::scene::eval::SceneFrame::for_instance(&inst);
    klipolo_model::scene::eval::eval_into(&inst, 0.0, &mut f);
    let before = ALLOCS.with(Cell::get);
    for i in 0..300 {
        klipolo_model::scene::eval::eval_into(&inst, i as f64 / 60.0, &mut f);
    }
    assert_eq!(ALLOCS.with(Cell::get) - before, 0);
}

/// Past every runtime cap at once: 1,500 repeated rects (1,000 drawn), a burst of 100,000 particles (500 written), four
/// 600-character texts (500 a node, 2,000 a scene), a 40-letter split text blurred whole (12 blurred units) and six
/// half-transparent groups (4 drawn offscreen). The evaluator builds and writes within its caps, allocates nothing once
/// warm, and counts what the renderer must degrade.
#[test]
fn a_scene_past_every_cap_evaluates_within_its_caps() {
    let long = "x".repeat(600);
    let texts: Vec<String> = (0..4)
        .map(|_| format!(r#"{{"type":"text","text":"{long}","box":[0,0,1,1]}}"#))
        .collect();
    let groups: Vec<&str> = (0..6)
        .map(|_| r#"{"type":"group","opacity":0.5,"children":[{"type":"rect","box":[0,0,1,1]}]}"#)
        .collect();
    let json = format!(
        r#"{{"scene":1,"duration":4,"nodes":[{texts},{groups},
          {{"type":"text","id":"blurred","text":"{letters}","split":"chars","blur":8,"box":[0,0,1,1]}},
          {{"type":"emitter","burst":100000,"life":3,"child":{{"type":"ellipse","box":[0,0,0.01,0.01]}}}},
          {{"type":"repeat","times":1500,"children":[{{"type":"rect","box":[0,0,0.01,0.01]}}]}}]}}"#,
        texts = texts.join(","),
        groups = groups.join(","),
        letters = "abcdefghij".repeat(4),
    );
    let s = std::sync::Arc::new(klipolo_model::scene::timeline::compile(
        &klipolo_model::scene::Scene::from_stored(json.as_bytes()).unwrap(),
    ));
    let inst = klipolo_model::scene::instance::SceneInstance::new(&s, &Default::default());
    let mut f = klipolo_model::scene::eval::SceneFrame::for_instance(&inst);
    klipolo_model::scene::eval::eval_into(&inst, 0.5, &mut f);
    let before = ALLOCS.with(Cell::get);
    for i in 0..60 {
        klipolo_model::scene::eval::eval_into(&inst, 0.5 + i as f64 / 60.0, &mut f);
    }
    assert_eq!(ALLOCS.with(Cell::get) - before, 0);
    let counts = f.counts();
    let caps = klipolo_model::scene::SceneLimits::MAX_INSTANCES;
    assert!(
        counts.instances <= caps && inst.dropped_instances() > 0,
        "{counts:?}"
    );
    assert!(f.particles().len() <= klipolo_model::scene::SceneLimits::MAX_PARTICLES);
    let over = counts.over();
    assert!(
        over.instances
            && over.particles
            && over.node_chars
            && over.scene_chars
            && over.blurring_units
            && over.isolated_groups,
        "{counts:?}"
    );
    assert_eq!(counts.isolated_groups, 6);
    assert_eq!(counts.blurring_units, 40);
    assert_eq!(counts.most_chars, 600);
}

/// Tracks are shared, one per tween and property, whatever the instances. A thousand instances under
/// 256 tweens of five properties each are refused by the check (`scene.tooManyTracks`); compiled anyway, the scene and
/// a layer of it stay small, where per-instance tracks made about 1.8 million of them.
#[test]
fn a_thousand_instances_under_256_tweens_are_refused_and_compile_small() {
    let tweens: Vec<&str> = (0..256)
        .map(|_| r#"{"to":".c","at":0,"dur":1,"x":0.1,"y":0.1,"opacity":0.5,"scale":1.2,"rotate":10}"#)
        .collect();
    let json = format!(
        r#"{{"scene":1,"duration":4,"nodes":[{{"type":"repeat","times":1000,"children":[{{"type":"rect","class":"c","box":[0,0,0.01,0.01]}}]}}],
          "timeline":[{}]}}"#,
        tweens.join(",")
    );
    let scene = klipolo_model::scene::Scene::from_stored(json.as_bytes()).unwrap();
    let problems =
        klipolo_model::scene::check_scene(&scene, klipolo_model::contrib::Catalog::builtin())
            .unwrap_err();
    assert!(
        problems.iter().any(|p| p.code == "scene.tooManyTracks"),
        "{problems:?}"
    );
    let before = BYTES.with(Cell::get);
    let s = std::sync::Arc::new(klipolo_model::scene::timeline::compile(&scene));
    let inst = klipolo_model::scene::instance::SceneInstance::new(&s, &Default::default());
    let used = BYTES.with(Cell::get) - before;
    println!("compile and one layer: {used} bytes allocated");
    assert!(used < 64 << 20, "{used} bytes");
    assert_eq!(inst.len(), 1001);
}

/// A frame of an instance past the track cap copies its settled still into the frame's own
/// space, allocating nothing once warm.
#[test]
fn a_still_frame_allocates_nothing_once_warm() {
    let tweens: Vec<&str> = (0..256)
        .map(|_| r#"{"to":".c","at":0,"dur":1,"x":0.1,"y":0.1,"opacity":0.5,"scale":1.2,"rotate":10}"#)
        .collect();
    let json = format!(
        r#"{{"scene":1,"duration":4,"nodes":[{{"type":"repeat","times":1000,"children":[{{"type":"rect","class":"c","box":[0,0,0.01,0.01]}}]}}],
          "timeline":[{}]}}"#,
        tweens.join(",")
    );
    let s = std::sync::Arc::new(klipolo_model::scene::timeline::compile(
        &klipolo_model::scene::Scene::from_stored(json.as_bytes()).unwrap(),
    ));
    let inst = klipolo_model::scene::instance::SceneInstance::new(&s, &Default::default());
    let mut f = klipolo_model::scene::eval::SceneFrame::for_instance(&inst);
    klipolo_model::scene::eval::eval_into(&inst, 0.0, &mut f);
    assert!(f.counts().still, "{:?}", f.counts());
    let before = ALLOCS.with(Cell::get);
    for i in 0..60 {
        klipolo_model::scene::eval::eval_into(&inst, i as f64 / 60.0, &mut f);
    }
    assert_eq!(ALLOCS.with(Cell::get) - before, 0);
}
