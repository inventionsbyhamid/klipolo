//! The preview test every caption style needs: one frame of a sample page, compared with its golden.
//! A new style file without a golden fails here; `UPDATE_GOLDEN=1` writes it. Look at the image before committing it.
//! Also: the export path draws what the preview does (run in the Linux gate), at two sizes for a glow blurred at half
//! size, and a glyph's ink past its advance is never cut.

use klipolo_model::captions::styles::BUILTIN_FILES;

const SAMPLE: [(&str, f64, f64); 4] = [
    ("Make", 1.0, 1.3),
    ("$500", 1.35, 1.7),
    ("with", 1.75, 1.95),
    ("this", 2.0, 2.3),
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn every_built_in_style_draws_like_its_golden() {
    klipolo_engine::render::fonts::configure_for_tests();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/captions");
    let goldens: std::collections::BTreeSet<String> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(|e| {
                e.ok()?
                    .file_name()
                    .to_str()?
                    .strip_suffix(".png")
                    .map(String::from)
            })
            .collect()
        })
        .unwrap_or_default();
    let keys: std::collections::BTreeSet<String> =
        BUILTIN_FILES.iter().map(|(k, _)| (*k).to_owned()).collect();
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    if !update {
        assert_eq!(
            goldens, keys,
            "every style needs tests/golden/captions/<key>.png (UPDATE_GOLDEN=1 writes it)"
        );
    }
    for (key, _) in BUILTIN_FILES {
        let img = klipolo_engine::test_support::render_caption_frame(
            &format!("caption.{key}"),
            &SAMPLE,
            1.5,
            (540, 960),
        )
        .await;
        let path = dir.join(format!("{key}.png"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            img.save(&path).unwrap();
            continue;
        }
        let want = image::open(&path).unwrap().to_rgba8();
        let (within, mean) = klipolo_engine::test_support::compare(&img, &want, 6);
        eprintln!(
            "{key}: {:.3} % within 6 of its golden, mean {mean:.3}",
            within * 100.0
        );
        assert!(
            within >= 0.995 && mean <= 1.5,
            "{key}: {:.2} % within, mean {mean:.2}",
            within * 100.0
        );
    }
}

/// Run in the Linux gate: each style's frame drawn through `ExportJob::create` (frozen `Captions`, the job's fonts
/// folder), the worker's start-up, context and renderer matches the preview's. Arcade again at 1080 × 1920, where its
/// glow's blur (σ about 21 px) runs at half size, so both paths are pinned there too.
#[tokio::test(flavor = "multi_thread")]
async fn export_and_preview_draw_the_same_caption_frame() {
    let Some(_gpu) = klipolo_engine::test_support::adapter_or_skip().await else {
        return;
    };
    klipolo_engine::render::fonts::configure_for_tests();
    let sizes = BUILTIN_FILES
        .iter()
        .map(|(key, _)| (*key, (540, 960)))
        .chain([("arcade", (1080, 1920))]);
    for (key, size) in sizes {
        let style = format!("caption.{key}");
        let preview =
            klipolo_engine::test_support::render_caption_frame(&style, &SAMPLE, 1.5, size).await;
        let (export, job) = klipolo_engine::test_support::render_caption_frame_via_export(
            &style, &SAMPLE, 1.5, size,
        )
        .await;
        // `configure` is process-wide and tests configure first, so check the folder the worker would use.
        let dirs = klipolo_engine::export::worker::font_dirs(&job);
        assert_eq!(
            dirs.bundled,
            klipolo_fonts::bundled::bundled_dir(),
            "{key}: the job records the bundled fonts folder"
        );
        let (within, mean) = klipolo_engine::test_support::compare(&export, &preview, 6);
        assert!(
            within >= 0.995 && mean <= 1.5,
            "{key} at {size:?}: export vs preview {:.2} % within, mean {mean:.2}",
            within * 100.0
        );
        assert!(
            preview.pixels().any(|p| p.0[0] > 200),
            "{key}: the frame has its captions"
        );
    }
}

/// The spread of channel `channel` across `cell` (the variance of its column sums' positions), px².
fn spread(a: &klipolo_engine::render::captions::TestAtlas, cell: [u32; 4], channel: usize) -> f64 {
    let [x, y, w, h] = cell;
    let cols: Vec<f64> = (x..x + w)
        .map(|c| (y..y + h).map(|r| f64::from(a.at(c, r, channel))).sum())
        .collect();
    let total: f64 = cols.iter().sum();
    let mean = cols
        .iter()
        .enumerate()
        .map(|(i, v)| i as f64 * v)
        .sum::<f64>()
        / total;
    cols.iter()
        .enumerate()
        .map(|(i, v)| (i as f64 - mean).powi(2) * v)
        .sum::<f64>()
        / total
}

/// Arcade's glow (A, the fill blurred at σ = 0.175 em) keeps its size to the text at 720 × 1280 (σ about 14 px)
/// and at 1080 × 1920 (about 21 px), both blurred at half size with the halving's own blur taken off:
/// measured on the GPU's output, σ² = var(A) − var(R).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_glow_keeps_its_size_to_the_text_at_two_sizes() {
    let mut per_em = Vec::new();
    for out in [(720, 1280), (1080, 1920)] {
        let a = klipolo_engine::render::captions::test_atlas_with(
            "caption.arcade",
            &["GG", "EZ"],
            None,
            out,
        )
        .await;
        let sigmas: Vec<f64> = a
            .cells
            .iter()
            .map(|&cell| {
                (spread(&a, cell, 3) - spread(&a, cell, 0)).max(0.0).sqrt() / f64::from(a.em)
            })
            .collect();
        eprintln!(
            "Arcade's glow at {out:?}: σ/em {sigmas:.3?} (em {:.1} px)",
            a.em
        );
        per_em.push(sigmas.iter().sum::<f64>() / sigmas.len() as f64);
    }
    let (small, large) = (per_em[0], per_em[1]);
    assert!(
        (large / small - 1.0).abs() < 0.05,
        "σ/em {small:.4} at 720 × 1280, {large:.4} at 1080 × 1920"
    );
    assert!(
        (small - 0.175).abs() < 0.175 * 0.2,
        "about half the style's 0.35 em blur: {small:.4}"
    );
}

/// A glyph's ink left of its advance (Rubik Black Italic's "j" reaches 0.17 em past it) is drawn in its cell,
/// never cut at the word's box.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn ink_past_a_words_advance_is_drawn_whole() {
    let a = klipolo_engine::render::captions::test_atlas_with(
        "caption.clean",
        &["jog"],
        Some(("Rubik", 900, true)),
        (1080, 1920),
    )
    .await;
    let [x, y, w, h] = a.cells[0];
    let left = x + ((w as f32 - a.boxes[0][0]) / 2.0).floor() as u32;
    let outside: u64 = (x..left)
        .flat_map(|c| (y..y + h).map(move |r| (c, r)))
        .map(|(c, r)| u64::from(a.at(c, r, 0)))
        .sum();
    assert!(outside > 255, "the j's tail left of its box: {outside}");
}

/// The render thread's cost of the captions per frame: the frame's state and the drawer's `prepare` for a kept page
/// over 1,000 frames, the first frame of a page (it asks the build thread), and how long until the page shows. Run in
/// release on lavapipe (CPU Vulkan): the figures are lavapipe's, not the Mac's.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU; run in release"]
async fn a_caption_frame_costs_little() {
    let mut d = klipolo_engine::test_support::caption_drawer().await;
    for style in [
        "caption.bold",
        "caption.karaoke",
        "caption.highlight",
        "caption.arcade",
    ] {
        let c = d.time_frames(style, 5, 1_000);
        eprintln!(
            "{style} (1080x1920, lavapipe), render thread per frame: state {:.1} µs, prepare {:.1} µs (slowest {:.1} µs); a new page's first frame {:.1} µs, shown {:.1} ms later (built on the build thread)",
            c.state_us, c.prepare_us, c.slowest_us, c.first_us, c.shown_after_ms
        );
        assert!(c.state_us < 1_000.0 && c.prepare_us < 5_000.0 && c.first_us < 20_000.0);
    }
    eprintln!(
        "caption atlases kept after four styles' pages: {} atlases, {} bytes",
        d.atlases(),
        d.atlas_bytes()
    );
}

/// A page's outlined silhouette is one glyph a letter (the letter grown by the outline): each of Bold's three words
/// takes three glyph areas (its fill; its outline, and its fill again over it), where the stamps took about 50.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_caption_outline_is_one_glyph_a_letter() {
    let a =
        klipolo_engine::render::captions::test_atlas("caption.bold", &["MAKE", "IT", "POP"]).await;
    assert_eq!(a.areas, 9, "three glyph areas a word");
}
