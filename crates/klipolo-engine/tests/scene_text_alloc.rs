//! Its own binary, with a per-thread counting allocator: once warm, a
//! frame of text that changes as it plays (a counter, a scramble settling into each hold, a typewriter) or moves (split
//! letters rising, a span's marker growing) evaluates, places and packs its pieces with no allocation and no layout.
//! The drawer's own CPU path (`TextProbe`, no GPU), from the scene's evaluation to the draws' list.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use klipolo_engine::render::scene::{TextProbe, text_layout_calls};

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

const COUNTER: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"n","type":"text","text":"0","box":[0.1,0.3,0.8,0.4],"size":160,"weight":800,"fill":"#FFFFFF","align":"center",
   "split":"chars","count":{"from":0,"to":10000,"dur":1,"separator":true,"prefix":"$","suffix":" fans"}}],
  "timeline":[{"from":"#n","at":0,"dur":0.3,"y":"20px","stagger":0.02}]}"##;

const SCRAMBLE: &str = r##"{"scene":1,"duration":3,"nodes":[
  {"id":"s","type":"text","text":"SUBSCRIBE","box":[0.1,0.3,0.8,0.4],"size":120,"fill":"#FFFFFF",
   "scramble":{"at":0,"dur":1.2,"seed":7},"holds":[{"t":1.5,"text":"SUBSCRIBED"}]}]}"##;

const TYPEWRITER: &str = r##"{"scene":1,"duration":3,"nodes":[
  {"id":"w","type":"text","text":"Typed one letter at a time","box":[0.05,0.3,0.9,0.4],"size":80,"fill":"#FFFFFF",
   "letterSpacing":4,"reveal":{"by":"chars","at":0,"dur":2},
   "animate":{"letterSpacing":[{"t":0,"v":20},{"t":2,"v":4}],"size":[{"t":0,"v":60},{"t":2,"v":80}]}}]}"##;

const LETTERS_AND_MARKER: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"m","type":"text","split":"chars","box":[0.05,0.3,0.9,0.4],"size":120,"weight":700,"fill":"#FFFFFF",
   "align":"center","spans":[{"text":"Make it "},
     {"text":"pop","fill":"#111111","background":{"color":"#FFE14D","padding":10,"radius":8}}]}],
  "timeline":[{"from":"#m","at":0.2,"dur":0.5,"scaleX":0,"opacity":0,"y":"30px","rotate":20,"stagger":0.06}]}"##;

#[test]
fn a_frame_of_changing_text_allocates_and_lays_out_nothing_once_warm() {
    for (name, json) in [
        ("counter", COUNTER),
        ("scramble", SCRAMBLE),
        ("typewriter", TYPEWRITER),
        ("letters and marker", LETTERS_AND_MARKER),
    ] {
        let mut probe = TextProbe::new(json, &Default::default(), (1280, 720), 720.0 / 1080.0)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut drawn = 0;
        for i in 0..10 {
            drawn += probe.frame(f64::from(i) / 60.0);
        }
        let (allocs, layouts) = (ALLOCS.with(Cell::get), text_layout_calls());
        for i in 0..300 {
            drawn += probe.frame(f64::from(i) / 100.0);
        }
        assert_eq!(
            ALLOCS.with(Cell::get) - allocs,
            0,
            "{name}: no allocation per frame"
        );
        assert_eq!(text_layout_calls(), layouts, "{name}: no layout per frame");
        assert!(drawn > 0, "{name} draws");
    }
}
