//! Scene text in motion on the CPU, through `TextProbe` and the drawer's own packing:
//! text at rest stands on whole pixels, and text that moves glides by fractions of a pixel, never in whole-pixel steps
//! against its card: a slow slide moves every frame, and an eased rise's tail settles smoothly.

/// The probe of `json` at 1280 × 720 (two thirds of a target pixel a 1080p pixel).
fn probe(json: &str) -> klipolo_engine::render::scene::TextProbe {
    klipolo_engine::render::scene::TextProbe::new(
        json,
        &Default::default(),
        (1280, 720),
        720.0 / 1080.0,
    )
    .unwrap()
}

/// The first unmoved-by-scale draw's top left at scene time `t`.
fn corner(p: &mut klipolo_engine::render::scene::TextProbe, t: f64) -> [f32; 2] {
    assert!(p.frame(t) > 0, "text drawn at {t}");
    p.rest_corners()[0]
}

/// A title sliding 7 px over 10 s (under 0.05 target pixels a frame of 0.1 s) moves on every frame, by less than a
/// pixel, and stands on whole pixels where it starts, at rest.
#[test]
fn a_slow_slide_moves_every_frame() {
    const SLIDE: &str = r##"{"scene":1,"duration":12,"nodes":[
      {"id":"t","type":"text","text":"Slide","box":[0.0137,0.4,0.5,0.2],"size":60}],
      "timeline":[{"to":"#t","at":0,"dur":10,"ease":"none","x":"7px"}]}"##;
    let mut p = probe(SLIDE);
    let start = corner(&mut p, 0.0);
    assert!(
        start.iter().all(|v| (v - v.round()).abs() < 1e-3),
        "at rest on whole pixels: {start:?}"
    );
    let mut last = start;
    for i in 1..100 {
        let now = corner(&mut p, f64::from(i) / 10.0);
        let dx = now[0] - last[0];
        assert!(
            dx > 1e-3 && dx < 1.0,
            "frame {i} moves a fraction of a pixel: {last:?} then {now:?}"
        );
        last = now;
    }
}

/// Letters rising 60 px with `power3.out`: in the ease's tail every frame still moves up, by less than a pixel, until
/// the letter rests.
#[test]
fn an_eased_rise_settles_smoothly() {
    const RISE: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"id":"t","type":"text","text":"Rise","split":"chars","box":[0.05,0.4,0.6,0.2],"size":60}],
      "timeline":[{"from":"#t","at":0,"dur":0.6,"ease":"power3.out","y":"60px"}]}"##;
    let mut p = probe(RISE);
    let mut last = corner(&mut p, 0.3);
    for i in 1..16 {
        let now = corner(&mut p, 0.3 + f64::from(i) / 60.0);
        let dy = last[1] - now[1];
        assert!(
            dy > 1e-4 && dy < 1.0,
            "step {i} glides up: {last:?} then {now:?}"
        );
        last = now;
    }
    let rest = corner(&mut p, 1.0);
    assert!(
        rest.iter().all(|v| (v - v.round()).abs() < 1e-3),
        "at rest on whole pixels: {rest:?}"
    );
}

/// A repeat's copies rest `index × gap` apart, and every copy's text stands on whole
/// pixels at rest, whether the gap is a fraction or pixels.
#[test]
fn repeat_copies_rest_on_whole_pixels() {
    for gap in ["0.0731", "\"37.3px\""] {
        let json = format!(
            r##"{{"scene":1,"duration":2,"nodes":[{{"type":"repeat","times":3,"gap":[0,{gap}],"children":[
              {{"id":"t","type":"text","text":"Row","box":[0.0137,0.118,0.5,0.06],"size":30}}]}}]}}"##
        );
        let mut p = probe(&json);
        assert_eq!(p.frame(1.0), 3, "three copies of one line ({gap})");
        let corners = p.rest_corners();
        assert_eq!(corners.len(), 3, "{gap}");
        for c in corners {
            assert!(
                c.iter().all(|v| (v - v.round()).abs() < 1e-3),
                "gap {gap}: a copy rests off whole pixels at {c:?}"
            );
        }
    }
}

/// The card (`41px` from the left) and the title (`53px`) of a scene laid out for a layer's largest scale.
const PX_CARD: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"card","type":"rect","box":["41px",0.6,0.4,0.2],"fill":"#202020"},
  {"id":"t","type":"text","text":"Title","box":["53px",0.62,0.35,0.1],"size":40,"fill":"#FFFFFF"}]}"##;

/// Laid out for a pulse's largest scale (1.04) and packed at each scale on the way (as a
/// stand-in, or the drawer's old rule, packs it), a px-positioned title keeps its place against its card: its
/// whole-pixel shift is measured at the text's own layout scale, so it never ticks by a pixel while the card glides.
#[test]
fn a_px_title_moves_in_step_with_its_card_under_a_pulse() {
    let laid = 720.0 / 1080.0 * 1.04;
    let mut p = klipolo_engine::render::scene::TextProbe::new(
        PX_CARD,
        &Default::default(),
        (1280, 720),
        laid as f32,
    )
    .unwrap();
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for i in 0..=40 {
        let s = 1.0 + f64::from(i) / 1000.0;
        let k = laid / s;
        assert!(p.frame_at_k(1.0, k) > 0);
        let title = p.first_text_corner().unwrap()[0];
        let card = p.shape_corner(0).unwrap()[0];
        let off = title - card - 12.0 * k;
        lo = lo.min(off);
        hi = hi.max(off);
    }
    assert!(
        hi - lo < 0.05,
        "the title against its card moves by {:.3} px over the pulse (0 is in step)",
        hi - lo
    );
}

/// A title whose box mixes pixels and fractions, packed at each scale of a
/// pop (from 0.85, past 1 and back) while laid out for its largest, takes the same whole-pixel shift every frame.
#[test]
fn under_a_pop_the_whole_pixel_shift_is_the_same_every_frame() {
    const MIXED: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"id":"t","type":"text","text":"Mixed","box":["53.3px",0.41,"301.7px",0.1],"size":40}]}"##;
    let reach: f64 = 1.06;
    let laid = 720.0 / 1080.0 * reach;
    let mut p = klipolo_engine::render::scene::TextProbe::new(
        MIXED,
        &Default::default(),
        (1280, 720),
        laid as f32,
    )
    .unwrap();
    let mut first = None;
    for s in [0.85, 0.9, 0.97, 1.03, 1.06, 1.04, 1.01, 1.0] {
        assert!(p.frame_at_k(1.0, laid / s * (s / reach).max(1.0)) > 0);
        let shift = p.rest_shift("t").unwrap();
        let want = *first.get_or_insert(shift);
        assert!(
            (shift[0] - want[0]).abs() < 1e-9 && (shift[1] - want[1]).abs() < 1e-9,
            "at scale {s} the shift is {shift:?}, not {want:?}"
        );
    }
}
