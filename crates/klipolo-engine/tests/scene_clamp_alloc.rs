//! Its own binary, with a per-thread counting allocator: a scene layer held to its envelope keeps its held settings
//! between frames, so planning it allocates no more than planning one inside its envelope (no copy of its settings a
//! frame; a layer can stay held all session until its scene is measured again).

mod support;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use klipolo_engine::plan::frame_plan;
use klipolo_engine::registry::Registry;
use klipolo_engine::registry::pack::LoadedContrib;
use klipolo_engine::registry::pack_scene::{VerifiedScene, compile_verified};
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::ItemContent;
use support::*;

struct Counting;
thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static A: Counting = Counting;

/// `$copies` dots in a row (a repeat's `times`, 1–400).
const DOTS: &str = r##"{"scene":1,"duration":2,
  "params":[{"key":"copies","label":"Copies","type":"int","min":1,"max":400,"default":3}],
  "nodes":[{"type":"repeat","id":"row","times":"$copies","gap":[0.002,0],"children":[
    {"type":"ellipse","box":[0,0.4,0.002,0.02],"fill":"#FFFFFF"}]}]}"##;
const CARD: &str = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","id":"r","box":[0,0,1,1],"fill":"#202020"}]}"##;

/// The allocations of a warm frame's plan: the dots (measured up to 120 copies) asking `copies`, beside a card scene
/// not checked yet, so every frame has a list of notes (the card's "checking") whether the dots add to it or not.
fn warm_plan_allocations(copies: f64) -> u64 {
    let dots = loaded_scene("scene.dots.v1", "Dots", DOTS);
    let card = loaded_scene("scene.card.v1", "Card", CARD);
    let verdict = light_verdict(range_envelope("copies", 1.0, 120.0));
    let base = Registry::with_definitions(&Registry::builtin(), [&dots.def, &card.def]);
    let scene_for = |c: &LoadedContrib| {
        let scene = (c.key == dots.key).then(|| compile_verified(c, base.catalog()))?;
        Some(VerifiedScene::new(scene.unwrap(), &verdict))
    };
    let registry = Arc::new(Registry::with_contributions(
        &base,
        [&dots, &card],
        &|_| None,
        &scene_for,
    ));
    let mut p = project_showing(&dots.def.id, 2.0, &[("copies", copies)]);
    let content = ItemContent::generator(card.def.id.clone(), Default::default());
    let card_layer = item("card", 0.0, 2.0, bx(0.0, 0.0, 1.0, 1.0), content);
    p.timeline
        .tracks
        .push(overlay_track("cards", vec![card_layer]));
    let ctx = ctx_with(p, registry);
    for _ in 0..3 {
        frame_plan(&ctx, secs(0.5), FormatKind::Landscape);
    }
    let before = ALLOCS.with(Cell::get);
    let plan = frame_plan(&ctx, secs(0.5), FormatKind::Landscape);
    let made = ALLOCS.with(Cell::get) - before;
    assert_eq!(plan.layers.len(), 1, "the dots, and the card not drawn");
    made
}

#[test]
fn a_held_scene_layer_plans_with_no_more_allocation_than_one_inside_its_envelope() {
    let inside = warm_plan_allocations(100.0);
    let held = warm_plan_allocations(400.0);
    assert_eq!(held, inside, "held: {held} allocations, inside: {inside}");
}
