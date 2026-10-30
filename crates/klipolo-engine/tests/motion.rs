//! Layer motion in the frame plan: presets per format, keys after presets, no allocation per frame, an
//! unknown preset named and drawn still, and where the editor shows a layer with an enter preset.

mod support;

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::cell::Cell;

use klipolo_engine::plan::{Layer, PlanContext, frame_plan};
use klipolo_model::contrib::builtin;
use klipolo_model::layout::BoxN;
use klipolo_model::motion::{Blend, Ease, Key, Motion, MotionKeys, MotionUse};
use klipolo_model::project::FormatKind::{self, Landscape, Portrait};
use klipolo_model::timeline::ItemContent;
use support::*;

/// Counts this thread's allocations, so tests running beside it on other threads don't change the count.
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: AllocLayout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: AllocLayout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn slide(from: &str) -> MotionUse {
    MotionUse {
        preset: builtin::motion("slide"),
        params: [(
            "from".into(),
            klipolo_model::params::ParamValue::Text(from.into()),
        )]
        .into(),
        duration_us: 1_000_000,
        ease: None,
    }
}

/// The engine fixture with one logo layer at 2–6 s, given `motion` and `blend`.
fn logo(motion: Option<Motion>, blend: Option<Blend>) -> klipolo_model::timeline::Item {
    let mut logo = item(
        "logo",
        2.0,
        4.0,
        bx(0.4, 0.4, 0.2, 0.2),
        ItemContent::Image {
            asset_id: "logo".into(),
        },
    );
    logo.layout.motion = motion;
    logo.layout.blend = blend;
    logo
}

fn with(logo: klipolo_model::timeline::Item) -> PlanContext {
    let mut p = project();
    p.timeline.tracks = vec![overlay_track("T", vec![logo])];
    ctx(p)
}

fn image_rect(c: &PlanContext, at: i64, format: FormatKind) -> BoxN {
    let plan = frame_plan(c, at, format);
    plan.layers
        .iter()
        .find_map(|l| {
            if let Layer::Image(i) = l {
                Some(i.dest)
            } else {
                None
            }
        })
        .expect("the logo draws")
}

#[test]
fn a_slide_in_from_the_top_starts_off_canvas_in_every_format_and_lands_on_its_place() {
    let c = with(logo(
        Some(Motion {
            enter: Some(slide("top")),
            ..Default::default()
        }),
        None,
    ));
    for format in [Landscape, Portrait] {
        let start = image_rect(&c, 2_000_000, format);
        assert!(
            start.y + start.h <= 1e-9,
            "{format:?}: fully above the canvas at the start: {start:?}"
        );
        let middle = image_rect(&c, 2_500_000, format);
        let rest = image_rect(&c, 3_500_000, format);
        assert!(start.y < middle.y && middle.y < rest.y, "{format:?}");
        close(image_rect(&c, 3_000_000, format), rest);
    }
}

#[test]
fn keys_add_to_presets_in_the_researchs_order() {
    let keys = MotionKeys {
        x: vec![
            Key {
                t_us: 0,
                v: 0.0,
                ease: None,
            },
            Key {
                t_us: 4_000_000,
                v: 0.2,
                ease: None,
            },
        ],
        ..Default::default()
    };
    let moving = with(logo(
        Some(Motion {
            keys: Some(keys),
            ..Default::default()
        }),
        None,
    ));
    let still = image_rect(&with(logo(None, None)), 4_000_000, Landscape);
    let moved = image_rect(&moving, 4_000_000, Landscape);
    assert!(
        (moved.x - still.x - 0.1).abs() < 1e-9,
        "halfway through the keys: +0.1 of the canvas"
    );
}

#[test]
fn planning_a_moving_layer_allocates_no_more_than_a_still_one() {
    let still = with(logo(None, None));
    let keys = MotionKeys {
        scale: vec![
            Key {
                t_us: 0,
                v: 1.0,
                ease: None,
            },
            Key {
                t_us: 4_000_000,
                v: 1.5,
                ease: None,
            },
        ],
        ..Default::default()
    };
    let moving = with(logo(
        Some(Motion {
            enter: Some(slide("left")),
            keys: Some(keys),
            ..Default::default()
        }),
        Some(Blend::Screen),
    ));
    let count = |c: &PlanContext| {
        let _ = frame_plan(c, 2_100_000, Landscape); // warms the string table
        let before = allocations();
        for t in (2_100_000..5_900_000).step_by(33_333) {
            std::hint::black_box(frame_plan(c, t, Landscape));
        }
        allocations() - before
    };
    assert_eq!(
        count(&moving),
        count(&still),
        "motion and blend add no allocation per frame"
    );
}

#[test]
fn an_unknown_preset_draws_the_layer_still_and_names_it() {
    let unknown = MotionUse {
        preset: builtin::motion("swirl"),
        ..slide("top")
    };
    let c = with(logo(
        Some(Motion {
            enter: Some(unknown),
            ..Default::default()
        }),
        None,
    ));
    close(
        image_rect(&c, 2_000_000, Landscape),
        image_rect(&with(logo(None, None)), 2_000_000, Landscape),
    );
    assert!(
        frame_plan(&c, 2_000_000, Landscape)
            .missing
            .iter()
            .any(|m| &*m.id == "klipolo.builtin/motion.swirl")
    );
}

#[test]
fn shown_from_waits_for_the_enter_preset() {
    let l = logo(
        Some(Motion {
            enter: Some(slide("top")),
            ..Default::default()
        }),
        None,
    );
    assert_eq!(klipolo_engine::plan::overlay::shown_from_us(&l), 3_000_000);
    assert_eq!(
        klipolo_engine::plan::overlay::shown_from_us(&logo(None, None)),
        2_000_000
    );
}

/// A preset's eased overshoot stays inside `reach`: pop's back.out grows the logo past its own size
/// and so does a springy pop's first swing, and the box never passes what effect
/// targets and decodes are sized for.
#[test]
fn reach_holds_a_pops_overshoot() {
    let springy = Ease::Spring {
        stiffness: 100.0,
        damping: 5.0,
        mass: 1.0,
    };
    for ease in [None, Some(springy)] {
        let pop = MotionUse {
            preset: builtin::motion("pop"),
            params: Default::default(),
            duration_us: 1_000_000,
            ease,
        };
        let m = Motion {
            enter: Some(pop),
            ..Default::default()
        };
        let reach = klipolo_engine::plan::motion::reach(&m);
        let c = with(logo(Some(m), None));
        let rest = image_rect(&with(logo(None, None)), 4_000_000, Landscape);
        let widest = (2_000_000..3_000_000)
            .step_by(20_000)
            .map(|t| image_rect(&c, t, Landscape).w)
            .fold(0.0, f64::max);
        assert!(widest > rest.w, "{ease:?} overshoots");
        assert!(
            widest <= rest.w * reach + 1e-12,
            "{ease:?}: {widest} > {} × {reach}",
            rest.w
        );
    }
}
