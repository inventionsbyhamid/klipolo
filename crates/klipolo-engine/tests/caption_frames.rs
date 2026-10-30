//! One caption frame: which words show, their colours and motion, the box that
//! follows the spoken word, and the block's tilt and exit, from each style's data. Pure: no GPU. A frame allocates
//! nothing (counted by this binary's own allocator, per thread).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use klipolo_engine::plan::captions::frame::{CaptionFrame, caption_frame};
use klipolo_engine::test_support::captions_project;
use klipolo_model::captions::captions;
use klipolo_model::captions::styles::CaptionStyles;
use klipolo_model::project::FormatKind;
use klipolo_model::time::secs_to_us as s;

/// Counts this thread's allocations, so the other tests running in the binary never move this test's count.
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: every call is forwarded to `System` unchanged; the count is a plain thread-local cell.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: the caller's contract is passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract is passed on.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: the caller's contract is passed on.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: the caller's contract is passed on.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// This thread's allocations so far.
fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

fn frame_at(style: &str, words: &[(&str, f64, f64)], t: f64) -> CaptionFrame {
    let (p, sp) = captions_project(style, words);
    let st = CaptionStyles::builtin()
        .for_settings(&p.captions, &p.settings.formats)
        .0;
    let c = captions(&p, &sp);
    let pages = &c.pages[&FormatKind::Portrait];
    let i = klipolo_model::captions::page_at(pages, s(t)).expect("a page");
    caption_frame(&st, pages, i, s(t))
}

const THREE: [(&str, f64, f64); 3] = [("make", 1.0, 1.3), ("it", 1.4, 1.6), ("pop", 1.7, 2.0)];

#[test]
fn bold_reveals_word_by_word_from_fifty_ms_early() {
    let f = frame_at("caption.bold", &THREE, 1.36);
    assert_eq!(
        f.words.iter().map(|w| w.visible).collect::<Vec<_>>(),
        [true, true, false],
        "'it' shows 50 ms before 1.4 s"
    );
    let popping = &f.words[1];
    assert!(
        popping.scale < 1.0 && popping.opacity < 1.0,
        "it is still popping in"
    );
}

#[test]
fn karaoke_lights_the_spoken_word_and_dims_the_upcoming() {
    let f = frame_at("caption.karaoke", &THREE, 1.5);
    assert_eq!(f.words[1].fill, [255, 212, 0], "#FFD400 on 'it'");
    assert_eq!(f.words[0].fill, [255, 255, 255]);
    assert!((f.words[2].fill_alpha - 0.55).abs() < 1e-6);
}

#[test]
fn highlight_glides_its_box_to_the_next_word() {
    let f = frame_at("caption.highlight", &THREE, 1.40);
    let b = f.word_box.expect("a word box");
    assert_eq!((b.from, b.to), (Some(0), 1));
    assert!(b.glide > 0.0 && b.glide < 1.0, "{}", b.glide);
    assert!(f.words[1].on_box);
    // Once there (100 ms), it still names the word before: the drawer never takes it for a line's first word and grows
    // it in again (its 150 ms growth isn't over yet).
    let f = frame_at("caption.highlight", &THREE, 1.47);
    let b = f.word_box.expect("a word box");
    assert_eq!((b.from, b.to, b.glide), (Some(0), 1, 1.0));
}

#[test]
fn loud_tilts_each_page_the_same_way_every_time() {
    let a = frame_at("caption.loud", &THREE, 1.9);
    let b = frame_at("caption.loud", &THREE, 1.9);
    assert_eq!(a.rotation_deg, b.rotation_deg, "seeded by the page's index");
    assert!(a.rotation_deg.abs() <= 3.0);
}

#[test]
fn clean_fades_out_only_before_a_gap() {
    let near = frame_at(
        "caption.clean",
        &[("One.", 0.0, 0.5), ("Two.", 0.85, 1.3)],
        0.82,
    );
    assert_eq!(
        near.opacity, 1.0,
        "the next page follows within 100 ms of the held end: a cut"
    );
    let gap = frame_at(
        "caption.clean",
        &[("One.", 0.0, 0.5), ("Two.", 4.0, 4.5)],
        0.80,
    );
    assert!(gap.opacity < 1.0, "fading before a gap");
}

/// Every style's frame, with a word the person coloured, highlighted and emphasised, from before
/// its lead to after its exit.
#[test]
fn a_frame_allocates_nothing() {
    use klipolo_model::captions::looks::{Look, WordLook};
    use klipolo_model::captions::styles::BUILTIN_FILES;

    for (key, _) in BUILTIN_FILES {
        let (mut p, sp) = captions_project(&format!("caption.{key}"), &THREE);
        p.word_looks = vec![WordLook {
            asset_id: "v".into(),
            start_us: s(1.4),
            end_us: s(1.6),
            look: Look {
                color: Some("#FF0000".into()),
                highlight: Some(true),
                emphasis: Some(true),
                ..Look::default()
            },
        }]
        .into();
        let st = CaptionStyles::builtin()
            .for_settings(&p.captions, &p.settings.formats)
            .0;
        let c = captions(&p, &sp);
        let pages = &c.pages[&FormatKind::Portrait];
        let before = allocations();
        for k in 0..1_000 {
            std::hint::black_box(caption_frame(&st, pages, 0, s(0.9 + f64::from(k) * 0.0015)));
        }
        assert_eq!(allocations(), before, "{key}");
    }
}
