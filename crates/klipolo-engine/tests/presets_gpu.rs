//! The shader presets drawn, as the render worker draws a draft alone
//! (`DraftLayer::builtin`: a generator over the whole frame, a filter on the worker's test card at 70 % of it):
//! - one golden each, landscape at 1.0 s, over the worker's mid-grey checkerboard (`UPDATE_GOLDEN=1` writes them; look
//!   at each before committing it). Film grain's is its 160 × 90 middle: noise compresses badly;
//! - each at 0, 1 and 2.5 s in every format: a generator's luma isn't flat, it moves from one time to the next, and it
//!   covers the frame; a filter keeps the card's coverage and covers nothing past its padding (glow and drop shadow draw
//!   into it); and the output check would replace nothing, read by the worker's audit of the preset as an agent's
//!   shader (no NaN, nothing out of range, no colour over its alpha), since a built-in draws without it. The audit runs
//!   at the defaults in every format, and in landscape also at each costly corner the worker would measure and at each
//!   number setting's either end.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::make::audit::{AuditDraw, Unsanitised, audit};
use klipolo_engine::make::draft::{DRAFT_SECONDS, DraftLayer};
use klipolo_engine::make::envelope::{Params, corners, costly_list};
use klipolo_engine::make::sizes::frame_size;
use klipolo_engine::make::stats::frame_stats;
use klipolo_engine::plan::pack_params;
use klipolo_engine::registry::presets::SHADER_BODIES;
use klipolo_engine::registry::{ShaderEffect, ShaderKind};
use klipolo_model::contrib::ContribId;
use klipolo_model::params::{ParamKind, ParamValue};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::secs_to_us;
use support::headless_alone;

/// The goldens' size: landscape, 640 px across.
const GOLDEN: (u32, u32) = (640, 360);
/// The part of its frame a noise preset's golden keeps (x, y, w, h): the middle 160 × 90.
const NOISE_CROP: (u32, u32, u32, u32) = (240, 135, 160, 90);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn every_shader_preset_draws_like_its_golden() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/presets");
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    if !update {
        let mut goldens: Vec<String> = std::fs::read_dir(&dir)
            .map(|d| {
                d.filter_map(|e| {
                    Some(
                        e.ok()?
                            .file_name()
                            .to_str()?
                            .strip_suffix(".png")?
                            .to_owned(),
                    )
                })
                .collect()
            })
            .unwrap_or_default();
        goldens.sort();
        let mut locals: Vec<&str> = SHADER_BODIES.iter().map(|(l, _)| *l).collect();
        locals.sort_unstable();
        assert_eq!(
            goldens, locals,
            "every preset needs tests/golden/presets/<local id>.png (UPDATE_GOLDEN=1 writes it)"
        );
    }
    let mut h = headless_alone().await;
    let tmp = tempfile::tempdir().unwrap();
    for (local, _) in SHADER_BODIES {
        let layer = preset(local, FormatKind::Landscape, tmp.path());
        let rgba = draw(&mut h, &layer, layer.project(), 1.0, GOLDEN).await;
        let mut img = over_checkerboard(&rgba, GOLDEN);
        if local == "generator.grain" {
            let (x, y, w, h) = NOISE_CROP;
            img = image::imageops::crop_imm(&img, x, y, w, h).to_image();
        }
        let path = dir.join(format!("{local}.png"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            img.save(&path).unwrap();
            continue;
        }
        let want = image::open(&path).unwrap().to_rgba8();
        let (within, mean) = klipolo_engine::test_support::compare(&img, &want, 6);
        assert!(
            within >= 0.995 && mean <= 1.5,
            "{local}: {:.2} % within, mean {mean:.2}",
            within * 100.0
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn every_shader_preset_draws_sound_frames_in_every_format() {
    let mut h = headless_alone().await;
    let tmp = tempfile::tempdir().unwrap();
    for format in [
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ] {
        let size = frame_size(format, 512);
        let px = f64::from(size.0.min(size.1)) / 1080.0;
        for (local, body) in SHADER_BODIES {
            let layer = preset(local, format, tmp.path());
            let checked = as_agent(&layer, body);
            // A filter's card alone: its coverage and its box.
            let card = match layer.kind {
                ShaderKind::Filter => {
                    let rgba = draw(&mut h, &layer, layer.baseline(), 1.0, size).await;
                    Some(Card::of(&rgba, size))
                }
                _ => None,
            };
            let mut prev: Option<Vec<f32>> = None;
            for t in [0.0, 1.0, 2.5] {
                let at = format!("{local} in {format:?} at {t} s");
                let rgba = draw(&mut h, &layer, layer.project(), t, size).await;
                let (stats, plane) = frame_stats(&rgba, prev.as_deref());
                prev = Some(plane);
                match &card {
                    None => {
                        assert!(
                            stats.luma.max - stats.luma.min > 0.02,
                            "{at}: flat {:?}",
                            stats.luma
                        );
                        // Every generator preset animates: each time draws another frame.
                        assert!(
                            stats.change_from_prev.is_none_or(|c| c > 0.001),
                            "{at}: the same as the last time ({:?})",
                            stats.change_from_prev
                        );
                        assert_eq!(
                            stats.alpha_coverage, 1.0,
                            "{at}: a generator fills its layer"
                        );
                    }
                    Some(card) => {
                        let pad = layer
                            .def
                            .padding
                            .as_ref()
                            .map_or(0.0, |p| f64::from(p.px_for(&BTreeMap::new(), &layer.def)))
                            * px;
                        card.holds(&rgba, size, pad, &at);
                        assert!(
                            stats.alpha_coverage >= card.coverage - card.edge_share(size),
                            "{at}: {} < the card's {}",
                            stats.alpha_coverage,
                            card.coverage
                        );
                        if matches!(local, "effect.glow" | "effect.drop-shadow") {
                            assert!(
                                card.outside(&rgba, size) > 0,
                                "{at}: nothing drawn past the card"
                            );
                        }
                    }
                }
                let settings = if format == FormatKind::Landscape {
                    audited(&checked, &layer)
                } else {
                    vec![Params::new()]
                };
                for values in settings {
                    let draw = AuditDraw {
                        effect: &checked,
                        params: pack_params(&checked, &values),
                        size,
                        at: t as f32,
                        duration: DRAFT_SECONDS as f32,
                        cache: None,
                    };
                    let replaced = audit(h.device(), &draw).await.unwrap();
                    assert_eq!(
                        replaced,
                        Unsanitised::default(),
                        "{at} at {values:?}: what the output check would replace"
                    );
                }
            }
        }
    }
}

/// The settings a preset's audit runs at: its defaults, each costly corner the worker would measure (as
/// the guard reads the preset under an agent's id), and each number setting at either end with the rest at their
/// defaults.
fn audited(checked: &ShaderEffect, layer: &DraftLayer) -> Vec<Params> {
    let facts = checked
        .facts
        .as_deref()
        .expect("a checked shader has its facts");
    let mut all = corners(&layer.def, &costly_list(&layer.def, &facts.costly_params));
    for spec in &layer.def.params {
        if let ParamKind::Float { min, max, .. } = spec.kind {
            for end in [min, max] {
                all.push(Params::from([(spec.key.clone(), ParamValue::Number(end))]));
            }
        }
    }
    all
}

/// The preset `local` as the worker's draft layer in `format`, at its defaults (its test card written in `tmp`).
fn preset(local: &str, format: FormatKind, tmp: &Path) -> DraftLayer {
    DraftLayer::builtin(&ContribId::builtin(local), BTreeMap::new(), format, tmp)
        .unwrap_or_else(|| panic!("{local} is a built-in shader"))
}

/// The preset under an agent's id, checked and hardened as an agent's shader is (its audit reads the hardened text).
fn as_agent(layer: &DraftLayer, body: &str) -> ShaderEffect {
    let mut def = (*layer.def).clone();
    let local = def.id.as_str().split_once('/').unwrap().1.to_owned();
    def.id = format!("lib.k3f9q2ma/{local}.v1").as_str().into();
    ShaderEffect::assemble_untrusted(&def, layer.kind, body)
        .unwrap_or_else(|d| panic!("{local}: {d:?}"))
}

/// One transparent frame of `project` at `t` s, as the worker draws a draft alone: RGBA, premultiplied, rows packed.
async fn draw(
    h: &mut Headless,
    layer: &DraftLayer,
    project: Project,
    t: f64,
    size: (u32, u32),
) -> Vec<u8> {
    layer.prepare(h, false, false);
    let f = h
        .render(Arc::new(project), secs_to_us(t), layer.format, size)
        .await
        .unwrap();
    assert!(f.warnings.is_empty(), "{}: {:?}", layer.def.id, f.warnings);
    let row = size.0 as usize * 4;
    f.bytes
        .chunks(f.stride as usize)
        .take(size.1 as usize)
        .flat_map(|r| &r[..row])
        .copied()
        .collect()
}

/// Premultiplied `rgba` over the worker's mid-grey checkerboard of 16 px squares, opaque.
fn over_checkerboard(rgba: &[u8], size: (u32, u32)) -> image::RgbaImage {
    image::RgbaImage::from_fn(size.0, size.1, |x, y| {
        let i = (y as usize * size.0 as usize + x as usize) * 4;
        let under = if (x / 16 + y / 16) % 2 == 0 {
            102.0
        } else {
            153.0
        };
        let a = f32::from(rgba[i + 3]) / 255.0;
        let px = |c: u8| (f32::from(c) + (1.0 - a) * under).round().clamp(0.0, 255.0) as u8;
        image::Rgba([px(rgba[i]), px(rgba[i + 1]), px(rgba[i + 2]), 255])
    })
}

/// The test card alone: the share of the frame it covers (alpha at least half, as `frame_stats` counts) and its box.
struct Card {
    coverage: f64,
    covered: Vec<bool>,
    /// x0, y0, x1, y1 (exclusive), in pixels.
    bounds: (u32, u32, u32, u32),
}

impl Card {
    fn of(rgba: &[u8], size: (u32, u32)) -> Self {
        let covered: Vec<bool> = rgba.chunks_exact(4).map(|p| p[3] >= 128).collect();
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (i, _) in covered.iter().enumerate().filter(|(_, c)| **c) {
            let (x, y) = (i as u32 % size.0, i as u32 / size.0);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
        assert!(x0 < x1 && y0 < y1, "the card is drawn");
        let coverage = covered.iter().filter(|c| **c).count() as f64 / covered.len() as f64;
        Self {
            coverage,
            covered,
            bounds: (x0, y0, x1, y1),
        }
    }

    /// Every pixel the card covers stays covered (but on its box's edge, which a padded layer draws through a grown
    /// target and may round its own way), and nothing is drawn further than `pad` pixels (plus 2 for the margin's
    /// rounding and filtering) from the card's box.
    fn holds(&self, rgba: &[u8], size: (u32, u32), pad: f64, at: &str) {
        let (x0, y0, x1, y1) = self.bounds;
        let lost = self
            .covered
            .iter()
            .zip(rgba.chunks_exact(4))
            .enumerate()
            .filter(|(i, (c, p))| {
                let (x, y) = (*i as u32 % size.0, *i as u32 / size.0);
                let inside = (x0 + 1..x1 - 1).contains(&x) && (y0 + 1..y1 - 1).contains(&y);
                inside && **c && p[3] < 128
            })
            .count();
        assert_eq!(lost, 0, "{at}: pixels of the card no longer covered");
        let reach = pad.ceil() as u32 + 2;
        let (gx0, gy0, gx1, gy1) = (
            x0.saturating_sub(reach),
            y0.saturating_sub(reach),
            x1 + reach,
            y1 + reach,
        );
        let stray = rgba
            .chunks_exact(4)
            .enumerate()
            .filter(|(i, p)| {
                let (x, y) = (*i as u32 % size.0, *i as u32 / size.0);
                p[3] > 0 && (!(gx0..gx1).contains(&x) || !(gy0..gy1).contains(&y))
            })
            .count();
        assert_eq!(
            stray, 0,
            "{at}: pixels drawn past the padding ({pad:.1} px)"
        );
    }

    /// The share of the frame on the edge of the card's box (one pixel wide).
    fn edge_share(&self, size: (u32, u32)) -> f64 {
        let (x0, y0, x1, y1) = self.bounds;
        f64::from(2 * (x1 - x0) + 2 * (y1 - y0)) / (f64::from(size.0) * f64::from(size.1))
    }

    /// Pixels drawn outside the card's box.
    fn outside(&self, rgba: &[u8], size: (u32, u32)) -> usize {
        let (x0, y0, x1, y1) = self.bounds;
        rgba.chunks_exact(4)
            .enumerate()
            .filter(|(i, p)| {
                let (x, y) = (*i as u32 % size.0, *i as u32 / size.0);
                p[3] > 0 && (!(x0..x1).contains(&x) || !(y0..y1).contains(&y))
            })
            .count()
    }
}
