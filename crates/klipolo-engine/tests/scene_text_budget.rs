//! Scene text's room and atlas, on the CPU through `TextProbe`, as the text build thread lays it out: text at rest
//! stands on whole pixels; every text a node can show is charged to the caps; plausible 4K text is drawn sharp; text
//! past the atlas's budget at its softest is refused and said (the atlas cap).

use std::time::{Duration, Instant};

/// Text at rest stands on whole target pixels wherever its box falls, so its cells are sampled texel
/// for pixel, not blurred by a fraction of one.
#[test]
fn text_at_rest_stands_on_whole_pixels() {
    const STILL: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"id":"t","type":"text","text":"Still title","box":[0.0137,0.0213,0.6,0.3],"size":61}]}"##;
    let mut probe = klipolo_engine::render::scene::TextProbe::new(
        STILL,
        &Default::default(),
        (1283, 721),
        721.0 / 1080.0,
    )
    .unwrap();
    assert!(probe.frame(1.0) > 0);
    let corners = probe.rest_corners();
    assert!(!corners.is_empty(), "at rest, unmoved");
    for c in &corners {
        assert!(c.iter().all(|v| (v - v.round()).abs() < 1e-3), "{c:?}");
    }
}

/// A crafted node: sixteen 500-character settings, each shown by a tween's text (200 tweens naming
/// them, as a shared project may; the checker counts the longest, 500, so it loads): every text laid out is charged,
/// so the node lays out at most its 500 characters and the rest is refused and said; quickly, within the atlas's
/// budget.
#[test]
fn every_text_a_node_lays_out_is_charged_to_the_caps() {
    let params: Vec<String> = (0..16)
        .map(|k| {
            let c = char::from(b'a' + k as u8);
            format!(
                r#"{{"key":"p{k}","label":"P","type":"text","default":"{}","maxLen":500}}"#,
                c.to_string().repeat(500)
            )
        })
        .collect();
    let tweens: Vec<String> = (0..200)
        .map(|i| {
            format!(
                r##"{{"to":"#n","at":{},"dur":0,"text":"$p{}"}}"##,
                0.005 * f64::from(i),
                (i + 3) % 16
            )
        })
        .collect();
    let json = format!(
        r#"{{"scene":1,"duration":4,"params":[{}],"nodes":[{{"id":"n","type":"text","text":"$p0","box":[0,0,1,1],
          "size":40,"split":"chars"}}],"timeline":[{}]}}"#,
        params.join(","),
        tweens.join(",")
    );
    let started = Instant::now();
    let probe = klipolo_engine::render::scene::TextProbe::new(
        &json,
        &Default::default(),
        (3840, 2160),
        2.0,
    )
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(probe.notes().cut, "the rest refused and said");
    assert!(probe.chars_laid_out() <= 500, "{}", probe.chars_laid_out());
    assert!(probe.atlas_bytes() <= klipolo_engine::render::scene::MAX_TEXT_ATLAS_BYTES);
}

/// What a 4K export of a kinetic title or a typewriter paragraph holds fits the atlas sharp: neither
/// is drawn softer.
#[test]
fn plausible_4k_text_is_drawn_sharp() {
    let title = "Every letter of this title rises on its own";
    assert_eq!(title.chars().count(), 43);
    let kinetic = format!(
        r#"{{"scene":1,"duration":2,"nodes":[{{"id":"t","type":"text","text":"{title}","split":"chars","box":[0,0.3,1,0.4],
          "size":150,"weight":800,"align":"center"}}]}}"#
    );
    let paragraph = "The quick brown fox jumps over the lazy dog. ".repeat(7);
    let typed = format!(
        r#"{{"scene":1,"duration":4,"nodes":[{{"id":"w","type":"text","text":"{}","box":[0.05,0.05,0.9,0.9],"size":64,
          "reveal":{{"by":"chars","dur":3}}}}]}}"#,
        paragraph.trim_end()
    );
    for (name, json) in [("kinetic title", kinetic), ("typewriter", typed)] {
        let probe = klipolo_engine::render::scene::TextProbe::new(
            &json,
            &Default::default(),
            (3840, 2160),
            2.0,
        )
        .unwrap();
        assert!(
            !probe.softer(),
            "{name} is drawn sharp ({} bytes)",
            probe.atlas_bytes()
        );
        assert!(!probe.notes().cut, "{name} isn't cut");
    }
}

/// The atlas cap: text that doesn't fit the atlas even at its softest is refused and said, never kept
/// over the budget: 960 letters at 2,048 px an em, which would hold about 23 MiB at a sixteenth of their size, and
/// 1,920, which wouldn't fit an atlas's sides there either.
#[test]
fn an_atlas_past_its_budget_at_its_softest_is_refused() {
    for nodes in [2usize, 4] {
        let h = 1.0 / nodes as f64;
        let node = |k: usize| {
            format!(
                r#"{{"type":"text","text":"{}","box":[0,{},1,{h}],"size":2000,"split":"chars"}}"#,
                "W".repeat(480),
                k as f64 * h
            )
        };
        let all: Vec<String> = (0..nodes).map(node).collect();
        let json = format!(r#"{{"scene":1,"duration":2,"nodes":[{}]}}"#, all.join(","));
        let probe = klipolo_engine::render::scene::TextProbe::new(
            &json,
            &Default::default(),
            (3840, 2160),
            2.0,
        )
        .unwrap();
        assert!(
            probe.refused(),
            "{nodes} nodes refused ({} bytes)",
            probe.atlas_bytes()
        );
        assert!(probe.notes().cut, "and said");
        assert!(probe.atlas_bytes() <= klipolo_engine::render::scene::MAX_TEXT_ATLAS_BYTES);
    }
}

/// A letter's blur is clamped in 1080p pixels, so a 4K frame blurs it as its preview does (twice the deviation in
/// texels, drawn small at twice the step) in the same scratch rows. Clamped in texels it was 24
/// at both.
#[test]
fn a_letters_blur_is_the_same_at_every_resolution() {
    let json = r##"{"scene":1,"duration":2,"nodes":[{"type":"text","text":"Soft","box":[0.1,0.3,0.6,0.3],"size":60,
      "fill":"#FFFFFF","blur":30}]}"##;
    let blurs = |size: (u32, u32), px: f32| {
        let mut probe =
            klipolo_engine::render::scene::TextProbe::new(json, &Default::default(), size, px)
                .unwrap();
        probe.frame(1.0);
        probe.blurs()
    };
    let (hd, hd_rows) = blurs((1920, 1080), 1.0);
    let (uhd, uhd_rows) = blurs((3840, 2160), 2.0);
    assert!(!hd.is_empty() && hd.len() == uhd.len(), "{hd:?} {uhd:?}");
    for (a, b) in hd.iter().zip(&uhd) {
        let ratio = b[0] / a[0];
        assert!((ratio - 2.0).abs() < 0.1, "4K {b:?} against 1080p {a:?}");
        assert_eq!(
            (a[1], b[1]),
            (8.0, 16.0),
            "steps: three to six texels a deviation"
        );
    }
    assert!(
        uhd_rows <= hd_rows && hd_rows <= 512,
        "rows {hd_rows} and {uhd_rows}"
    );
}
