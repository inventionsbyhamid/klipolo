//! Its own binary, with a per-thread counting allocator (performance first): once warm, a frame of a scene
//! that draws offscreen groups (faded, blurred, tilted, masked), a backdrop, generator fills and live particles
//! evaluates, places and packs its draws with no allocation: what goes offscreen, the masks' segments, the fills' and
//! backdrops' uses and every particle land in room made with the instance. The drawer's own CPU path (`TextProbe`,
//! no GPU, no text).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use klipolo_engine::render::scene::TextProbe;

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

/// Every group feature at once, moving: a faded group with a masked one inside, a tilted card, a blurred star, a
/// backdrop, two generator fills, a burst and a rate of particles, and a stroked path at half opacity.
const BUSY: &str = r##"{"scene":1,"duration":4,"nodes":[
  {"type":"ellipse","id":"spot","box":[0.1,0.1,0.3,0.3],"fill":"#FFFFFF","opacity":0},
  {"type":"group","id":"fade","box":[0,0,1,1],"opacity":0.6,"children":[
    {"type":"group","box":[0,0,1,1],"mask":{"node":"#spot","mode":"luma"},"children":[
      {"type":"rect","box":[0.05,0.05,0.4,0.4],"fill":"#FF3B30"}]},
    {"type":"group","id":"tilt","box":[0.5,0.1,0.4,0.4],"rotateY":30,"children":[
      {"type":"rect","box":[0,0,1,1],"fill":{"generator":"klipolo.builtin/generator.gradient"}}]}]},
  {"type":"star","id":"star","box":[0.1,0.55,0.2,0.3],"blur":4,"fill":"#FFD200"},
  {"type":"backdrop","box":[0.4,0.55,0.25,0.3],"radius":16,"blur":10,"tint":"rgba(255,255,255,0.2)"},
  {"type":"ellipse","box":[0.7,0.55,0.2,0.3],"fill":{"generator":"klipolo.builtin/generator.gradient"}},
  {"type":"emitter","box":[0.5,0.5,0.05,0.05],"burst":80,"rate":40,"life":[1,2],"speed":[100,300],"gravity":300,
   "child":{"type":"star","box":[0,0,"12px","12px"],"fill":"#00FFAA"}},
  {"type":"path","box":[0.05,0.05,0.2,0.2],"d":"M0 0 L10 0 L10 10 Z","fill":"#00F","stroke":"#FFF","strokeWidth":2,
   "opacity":0.5}],
  "timeline":[{"to":"#tilt","at":0,"dur":4,"rotateY":-30},{"to":"#fade","at":0,"dur":4,"opacity":0.9},
    {"to":"#star","at":0,"dur":4,"blur":12,"rotate":90}]}"##;

#[test]
fn a_frame_of_groups_backdrops_fills_and_particles_allocates_nothing_once_warm() {
    let mut probe = TextProbe::without_text(BUSY, (1280, 720), 720.0 / 1080.0, true).unwrap();
    for i in 0..10 {
        probe.frame(0.5 + f64::from(i) / 60.0);
    }
    let (isos, fills, backdrops, shapes) = probe.drawn_parts();
    assert!(
        isos >= 4,
        "groups, the tilt, the blurred star and the path go offscreen: {isos}"
    );
    assert_eq!((fills, backdrops), (2, 1));
    assert!(shapes > 80, "the particles draw: {shapes} shapes");
    let before = ALLOCS.with(Cell::get);
    for i in 0..300 {
        probe.frame(0.7 + f64::from(i) / 100.0);
    }
    let after = ALLOCS.with(Cell::get);
    assert_eq!(
        after - before,
        0,
        "300 warm frames allocated {}",
        after - before
    );
}
