//! Overlay tracks: images, videos, text, generators and adjustment layers, in track order, each
//! moved by its motion (`plan::motion`) and mixed with what is under it by its blend mode.

mod parts;

use klipolo_model::layout::{BoxN, Layout, resolve_rect};
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::time::{Us, unscaled};
use klipolo_model::timeline::{Item, ItemContent, PieceTime, piece_time, seed_id};

use super::effects::{self, Clock};
use super::framing::{FULL, area_aspect, aspect_of, canvas, cover_rect, fit_rect, is_whole_frame};
use super::generator;
use super::layers::MissingContrib;
use super::motion::{self, Moved};
use super::strings::Table;
use super::{
    AdjustmentLayer, GRAIN_FPS, GeneratorLayer, ImageLayer, Layer, PlanContext, SceneLayer,
    TextLayer, TextOwner, VideoLayer, VideoRole,
};
use crate::registry::ShaderKind;
use klipolo_model::contrib::{ContribKind, Resolution};
use parts::{blur_in, fills_animate, has_backdrop, note_unknown, typed};

/// The layers of every overlay track the context's solo shows (not hidden; soloed while any overlay track is), bottom
/// to top.
pub fn overlay_layers(
    ctx: &PlanContext,
    at_us: Us,
    format: FormatKind,
    s: &mut Table,
    missing: &mut Vec<MissingContrib>,
    out: &mut Vec<Layer>,
) {
    let (p, registry) = (&*ctx.project, &*ctx.registry);
    let primary = p.settings.primary_format;
    for track in p.timeline.tracks.iter().filter(|t| ctx.solo.shows(t)) {
        // Items are sorted by start and never overlap: only the last one starting at or before `at_us` can show.
        let next = track.items.partition_point(|i| i.start_us <= at_us);
        let Some(item) = next
            .checked_sub(1)
            .map(|i| &track.items[i])
            .filter(|i| at_us < i.end_us())
        else {
            continue;
        };
        let rect = resolve_rect(&item.layout, primary, format);
        let l = &item.layout;
        let local_us = at_us - item.start_us;
        // Seeded from the layer a split piece came from, so its patterns continue across the cut.
        let seed = seed_id(item);
        let clock = Clock {
            local_us,
            duration_us: item.duration_us,
            fps: p.settings.fps,
        };
        // Empty (and allocation-free) for layers without effects.
        let mut fx = effects::effect_passes(registry, seed, &item.effects, clock, s, missing);
        // A layer that doesn't move costs one `Option` check here.
        let motion = l.motion.as_ref();
        if let Some(m) = motion {
            note_unknown(m, registry, s, missing);
        }
        // The box `rest` after the layer's motion at this instant (as it is, for a layer that doesn't move).
        let moved = |rest: BoxN| {
            motion.map_or(Moved::still(rest), |m| {
                motion::apply(m, registry, local_us, item.duration_us, rest)
            })
        };
        let reach = |mv: &Moved| motion.map_or(1.0, |m| mv.reach_from_here(m));
        let blend = l.blend_mode();
        match &item.content {
            ItemContent::Image { asset_id } => {
                let Some(asset) = p.asset(asset_id).filter(|a| a.kind == AssetKind::Image) else {
                    continue;
                };
                let mv = moved(picture_rect(rect, asset, format));
                blur_in(registry, &item.id, &mv, clock, &mut fx);
                out.push(Layer::Image(ImageLayer {
                    id: s.str(&item.id),
                    asset_id: s.str(&asset.id),
                    path: s.path(&asset.path, registry),
                    dest: mv.rect,
                    rotation: l.rotation + mv.rotation,
                    opacity: l.opacity * mv.opacity,
                    radius: l.radius,
                    pad_px: effects::pad_of(&fx),
                    effects: fx,
                    blend,
                    reach: reach(&mv),
                }));
            }
            ItemContent::Video {
                asset_id,
                source_in_us,
                speed,
                mirror,
                ..
            } => {
                let Some(asset) = p.asset(asset_id).filter(|a| a.kind == AssetKind::Video) else {
                    continue;
                };
                let time = source_in_us + unscaled(local_us, *speed);
                let mv = moved(contain(rect, asset, format));
                blur_in(registry, &item.id, &mv, clock, &mut fx);
                out.push(Layer::Video(VideoLayer {
                    role: VideoRole::Overlay,
                    id: s.str(&item.id),
                    source: super::clip::source_at(asset, time, &ctx.media, registry, s),
                    dest: mv.rect,
                    crop: FULL,
                    rotation: l.rotation + mv.rotation,
                    opacity: l.opacity * mv.opacity,
                    radius: l.radius,
                    // A separated camera's own; the compositor flips the picture, never the file.
                    mirror: *mirror,
                    border: false,
                    look: None,
                    pad_px: effects::pad_of(&fx),
                    effects: fx,
                    blend,
                    reach: reach(&mv),
                }));
            }
            ItemContent::Text { text, style } => {
                let (w, h) = canvas(format);
                let mv = moved(rect);
                blur_in(registry, &item.id, &mv, clock, &mut fx);
                // The typewriter preset cuts the text while it types itself in or out: a new string only then.
                let shown = typed(text, mv.reveal);
                out.push(Layer::Text(TextLayer {
                    id: s.str(&item.id),
                    owner: TextOwner::Layer,
                    text: if shown.len() < text.len() {
                        shown.into()
                    } else {
                        s.str(text)
                    },
                    style: s.style(&item.id, style),
                    font_px: style.size * w.min(h) / 1080.0 * text_scale(l, primary, format),
                    // The box keeps its size, so the text is laid out once; the quad draws it `scale` times as big.
                    dest: mv.unscaled(rect),
                    rotation: l.rotation + mv.rotation,
                    opacity: l.opacity * mv.opacity,
                    scale: mv.scale,
                    reveal: mv.reveal,
                    pad_px: effects::pad_of(&fx),
                    effects: fx,
                    blend,
                    // Its box keeps its size, so its reach is the motion's own (which keys that only move or
                    // fade don't change).
                    reach: motion.map_or(1.0, motion::reach),
                    language: ctx.text_language.clone(),
                }));
            }
            ItemContent::Generator {
                source,
                params,
                still,
                ..
            } => {
                let scene = registry.scene(source);
                let shader = registry
                    .shader(source)
                    .filter(|sh| sh.kind == ShaderKind::Generator);
                if scene.is_none() && shader.is_none() {
                    // A library or project scene waits for this computer's check as a shader does.
                    let kind = if source.is_scene() {
                        ContribKind::Scene
                    } else {
                        ContribKind::Generator
                    };
                    effects::note_not_drawn(registry, missing, kind, source, s);
                    continue;
                }
                // Its passes, and its filters', follow its piece of the contribution's time (a split sparkle
                // continues too); the motion's blur-in keeps the layer's own time.
                let piece = piece_time(item).unwrap_or(PieceTime {
                    offset_us: 0,
                    span_us: item.duration_us,
                });
                let grain = f64::from(GRAIN_FPS);
                for pass in &mut fx {
                    generator::follow_piece(pass, piece, local_us, grain);
                }
                let mv = moved(rect);
                blur_in(registry, &item.id, &mv, clock, &mut fx);
                let (pad_px, reach) = (effects::pad_of(&fx), reach(&mv));
                let (rotation, opacity) = (l.rotation + mv.rotation, l.opacity * mv.opacity);
                if let Some(scene) = scene {
                    let scene_of = (source, &**scene);
                    let inputs =
                        super::scene::inputs(s, &item.id, scene_of, params, p, registry, missing);
                    out.push(Layer::Scene(SceneLayer {
                        id: s.str(&item.id),
                        dest: mv.rect,
                        rotation,
                        opacity,
                        radius: l.radius,
                        effects: fx,
                        blend,
                        reach,
                        pad_px,
                        t: super::scene::scene_time(scene, piece, local_us, *still),
                        fills_animate: fills_animate(scene, registry),
                        scene: scene.clone(),
                        contrib: s.str(source.as_str()),
                        params: inputs.params,
                        params_hash: inputs.hash,
                        images: inputs.images,
                        language: ctx.text_language.clone(),
                        motion_reach: motion.map_or(1.0, motion::reach),
                        fills: super::scene::SceneFills::of(scene, registry, s, missing),
                        blur: Some(registry.blur().clone()),
                        backdrop: has_backdrop(scene),
                        turns: motion.is_some_and(motion::turns),
                    }));
                    continue;
                }
                let shader = shader.expect("a scene or a generator shader, checked above");
                let ids = (seed, "source");
                let mut own = effects::pass(registry, shader, params, ids, clock, (s, missing));
                generator::follow_piece(&mut own, piece, local_us, grain);
                out.push(Layer::Generator(GeneratorLayer {
                    id: s.str(&item.id),
                    dest: mv.rect,
                    rotation,
                    opacity,
                    radius: l.radius,
                    cache_key: generator::cache_key(&own),
                    // `auto` is drawn at full size until the render worker resolves it.
                    half: shader.resolution == Resolution::Half,
                    source: own,
                    pad_px,
                    effects: fx,
                    blend,
                    reach,
                }));
            }
            ItemContent::Adjustment => {
                if !fx.is_empty() {
                    // It moves and fades like any layer; its box stays upright.
                    let mv = moved(rect);
                    out.push(Layer::Adjustment(AdjustmentLayer {
                        id: s.str(&item.id),
                        dest: mv.rect,
                        opacity: l.opacity * mv.opacity,
                        effects: fx,
                    }));
                }
            }
            ItemContent::Audio { .. } | ItemContent::Zoom { .. } => {}
        }
    }
}

/// When `item` is first drawn whole: once its enter preset has run (every preset starts hidden or off-canvas),
/// otherwise at its start. The editor puts the playhead here to show an item (a new title, a picked layer), so a paused
/// preview never sits on a frame the enter hides. Mirrored by `shownFromUs` in `apps/desktop/src/lib/timeline.ts` and
/// pinned by `tests/parity.rs`.
pub fn shown_from_us(item: &Item) -> Us {
    item.layout
        .motion
        .as_ref()
        .and_then(|m| m.enter.as_ref())
        .map_or(item.start_us, |u| {
            item.start_us + u.duration_us.min(item.duration_us)
        })
}

/// How much bigger a text layer's font is drawn in `format` than its size says. Text scales with its box: `size` is its
/// font in the primary format's box, and a format whose box is taller in pixels (its own placement, or a derived one
/// the canvas squeezes) draws it bigger by the same ratio. 1 in the primary format and wherever the box is derived
/// unsqueezed. Mirrored by `textScale` in `apps/desktop/src/lib/layout.ts` and pinned by `tests/parity.rs`.
pub fn text_scale(layout: &Layout, primary: FormatKind, format: FormatKind) -> f64 {
    let (pw, ph) = canvas(primary);
    let (fw, fh) = canvas(format);
    let base = resolve_rect(layout, primary, primary).h * ph;
    if base.is_nan() || base <= 0.0 {
        return 1.0;
    }
    let here = resolve_rect(layout, primary, format).h * fh;
    here / base * (pw.min(ph) / fw.min(fh))
}

/// The asset's shape, centred in its layout box (like the DOM stage's object-contain).
/// Mirrored by `containRect` in `apps/desktop/src/lib/layout.ts` (gizmo boxes) and pinned by `tests/parity.rs`.
pub fn contain(rect: BoxN, asset: &Asset, format: FormatKind) -> BoxN {
    let target = area_aspect(rect, format);
    fit_rect(rect, aspect_of(asset.width, asset.height, target), format)
}

/// Where a picture layer of `asset` draws in its layout box `rect` in `format`: contained in its box (`contain`), except
/// a box that is the whole frame (a title card's background, `Layout::full_frame`), which the picture covers, its
/// shape kept and what reaches past the frame cut off (`cover_rect`), so it fills every format with no footage round
/// it. Mirrored by `pictureRect` in `apps/desktop/src/lib/layout.ts` (gizmo boxes) and pinned by `tests/parity.rs`.
pub fn picture_rect(rect: BoxN, asset: &Asset, format: FormatKind) -> BoxN {
    if !is_whole_frame(rect) {
        return contain(rect, asset, format);
    }
    let target = area_aspect(rect, format);
    cover_rect(rect, aspect_of(asset.width, asset.height, target), format)
}
