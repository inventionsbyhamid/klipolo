//! An instance's text laid out at one size, on the CPU: every text node (`text_nodes.rs`) in paint order
//! charging what it lays out to the scene's room, the units each split text laid out (for the
//! instance, `set_units_many`), and every piece drawn into the atlas's pixels (`text_atlas.rs`), or refused when even
//! its softest drawing is past the budget. Run on the text build thread (`text_worker.rs`), never the render thread;
//! the GPU side is `text_cache.rs`'s.

use std::sync::Arc;

use klipolo_model::scene::SceneLimits;
use klipolo_model::scene::instance::SceneInstance;

use super::Drawn;
use super::glyph_sets::GlyphSet;
use super::place::sizes;
use super::statics::{Look, NodeStatic};
use super::text::{MAX_EM_PX, TextNotes, TextUnits};
use super::text_atlas::{AtlasImage, Cell, rasterize};
use super::text_nodes::{MAX_TEXTS, NodeText, Room, node_text};
use super::text_pipeline::{TextDraw, TextPaintDraw};
use crate::render::fonts;
use crate::render::shared_fonts::SharedFonts;

/// The most bytes one instance's text holds at one size, its atlas, draws and layouts together (its format's whole
/// budget, `text_cache.rs`).
pub(super) const ATLAS_BUDGET: u64 = 16 << 20;

/// An instance's text nodes laid out, which instance node each is, their cells in the atlas, what they tell, the most
/// draws a frame of them takes, the units each split text laid out, and the pixels per 1080p pixel they were laid out
/// at.
pub(super) struct TextLayouts {
    pub nodes: Vec<NodeText>,
    /// Per instance node, its text's index in `nodes` (`u32::MAX`: none).
    pub by_node: Vec<u32>,
    pub cells: Vec<Cell>,
    pub notes: TextNotes,
    /// Its atlas is drawn below full resolution for the budget.
    pub softer: bool,
    pub most: usize,
    /// The most paints a frame of them names (one a run of a look, not one a piece).
    pub paints: usize,
    /// (instance node, units) for every split text: the instance places its tweens on them.
    pub units: Vec<(usize, usize)>,
    pub k: f64,
    /// The atlas's size in texels (where a blurring piece's cell is read).
    pub atlas: (u32, u32),
    /// The baked shadows' cells, and where the colour plane lies in the atlas (`TextPaintDraw::cmap`).
    pub shadows: Vec<Cell>,
    pub cmap: [f32; 4],
}

/// What laying an instance's text out made: its layouts, and its atlas's pixels (None when refused for the budget).
pub(super) struct Laid {
    pub layouts: TextLayouts,
    pub image: Option<AtlasImage>,
}

impl TextLayouts {
    /// No text (an instance whose text nodes draw nothing), laid out at `k`.
    pub(super) fn empty(k: f64) -> Self {
        Self {
            nodes: Vec::new(),
            by_node: Vec::new(),
            cells: Vec::new(),
            notes: TextNotes::default(),
            softer: false,
            most: 0,
            paints: 0,
            units: Vec::new(),
            k,
            atlas: (1, 1),
            shadows: Vec::new(),
            cmap: [0.0; 4],
        }
    }

    /// The text of instance node `i`, if it has one.
    pub(super) fn node(&self, i: usize) -> Option<&NodeText> {
        let k = *self.by_node.get(i)?;
        self.nodes.get(k as usize)
    }

    /// About how many CPU bytes it holds: its pieces, unit boxes, lines, cells and the rest.
    pub(super) fn bytes(&self) -> u64 {
        use std::mem::size_of;
        let units = |t: &TextUnits| {
            t.pieces.capacity() * size_of::<super::text::Piece>()
                + t.unit_rects.capacity() * 16
                + t.lines.capacity() * 16
                + t.line_glyphs.capacity() * 2
                + t.text.capacity()
        };
        let nodes: usize = self
            .nodes
            .iter()
            .map(|n| {
                n.layouts.iter().map(units).sum::<usize>()
                    + n.set.as_ref().map_or(0, |s| units(s.units()) + 1024)
                    + n.layout_of.capacity() * 2
                    + n.look_boxes.capacity() * 16
                    + n.cells.capacity() * 4
                    + size_of::<NodeText>()
            })
            .sum();
        let cells = (self.cells.capacity() + self.shadows.capacity()) * size_of::<Cell>();
        (nodes + cells + self.by_node.capacity() * 4) as u64
    }
}

/// Whether instance nodes with these statics draw text.
pub(super) fn has_text(statics: &[NodeStatic]) -> bool {
    statics
        .iter()
        .any(|s| matches!(s.look, Look::Text | Look::Emoji))
}

/// `inst`'s text (its nodes' statics `statics`) laid out where it is `drawn` (its em at most `max_px`), its CJK in
/// `language`'s faces, and the atlas's pixels (its baked shadows' image at most `max_side` a side: the device's).
/// None when it has no text.
pub(super) fn lay_out_all(
    fonts: &mut SharedFonts,
    language: &Arc<str>,
    inst: &mut SceneInstance,
    statics: &[NodeStatic],
    (drawn, max_px): (Drawn, f32),
    max_side: u32,
) -> Option<Laid> {
    if !has_text(statics) {
        return None;
    }
    super::text_counts::count_build();
    let index = fonts::index();
    let mut boxes = Vec::with_capacity(inst.len());
    sizes(inst, statics, drawn, &mut boxes);
    let px = drawn.k as f32;
    let mut room = Room {
        chars: SceneLimits::MAX_TEXT_SCENE,
        texts: MAX_TEXTS,
        max_px: max_px.min(MAX_EM_PX),
    };
    let mut notes = TextNotes::default();
    let mut nodes = Vec::new();
    let mut by_node = vec![u32::MAX; inst.len()];
    let mut units = Vec::new();
    let mut paints_most = 0usize;
    for (i, st) in statics.iter().enumerate() {
        if !matches!(st.look, Look::Text | Look::Emoji) {
            continue;
        }
        let (w, h) = boxes.get(i).copied().unwrap_or((0.0, 0.0));
        let box_px = (w as f32, h as f32);
        let laid = node_text(
            fonts, &index, inst, i, box_px, px, language, &mut room, &mut notes,
        );
        let Some(mut nt) = laid else {
            continue;
        };
        // The paints a frame of it names: a run of each look, its own text's and a glyph set's, and
        // each background; never more than its draws.
        let lines = nt.layouts.iter().map(|l| l.lines.len().max(1)).max();
        let backgrounds = nt.paint.backgrounds.iter().flatten().count() * lines.unwrap_or(1);
        let mut paints = (2 * nt.paint.fills.len().max(1) + backgrounds).min(nt.most.max(1));
        if st.particle {
            // An emoji particle: its one laid-out text drawn at every live particle.
            nt.most = nt.most.saturating_mul(inst.particle_capacity().max(1));
            paints = paints.saturating_mul(inst.particle_capacity().max(1));
        }
        paints_most += paints;
        units.push((i, nt.units));
        by_node[i] = nodes.len() as u32;
        nodes.push(nt);
    }
    inst.set_units_many(&units);
    // Every layout's pieces, then the set's, node after node: each layout's first cell noted.
    let mut next = 0u32;
    for nt in &mut nodes {
        let set = nt.set.as_ref().map(GlyphSet::units);
        let counts: Vec<u32> = nt
            .layouts
            .iter()
            .chain(set)
            .map(|l| l.pieces.len() as u32)
            .collect();
        for n in counts {
            nt.cells.push(next);
            next += n;
        }
    }
    let most: usize = nodes.iter().map(|n| n.most).sum();
    let mut layouts = TextLayouts {
        most,
        paints: paints_most,
        nodes,
        by_node,
        cells: Vec::new(),
        notes,
        softer: false,
        units,
        k: drawn.k,
        atlas: (1, 1),
        shadows: Vec::new(),
        cmap: [0.0; 4],
    };
    // The atlas, the draws' and paints' buffers (and their copies) and the layouts share the budget.
    let draws = (2 * most * std::mem::size_of::<TextDraw>()
        + 2 * paints_most * std::mem::size_of::<TextPaintDraw>()) as u64;
    let budget = ATLAS_BUDGET.saturating_sub(draws + layouts.bytes());
    // An outline is at most half the em and 256 px (its stroke is rasterised beside every glyph).
    let texts: Vec<(&TextUnits, f32)> = layouts
        .nodes
        .iter()
        .flat_map(|nt| {
            let outline = nt.paint.outline.map_or(0.0, |(_, w)| w * px);
            let set = nt.set.as_ref().map(GlyphSet::units);
            nt.layouts.iter().chain(set).map(move |l| {
                let w = (outline * l.scale).min(l.font_px / 2.0).clamp(0.0, 256.0);
                (l, w)
            })
        })
        .collect();
    let image = rasterize(fonts.system(), &texts, budget);
    drop(texts);
    let Some(mut image) = image else {
        // Past the budget even at its softest: refused, and said (the atlas cap). Nothing of it draws.
        layouts.notes.cut = true;
        layouts.nodes.clear();
        layouts.by_node.clear();
        return Some(Laid {
            layouts,
            image: None,
        });
    };
    bake_shadows(&mut layouts, &mut image, px, (budget, max_side));
    // The glyphs were for the atlas only: a frame draws pieces.
    for nt in &mut layouts.nodes {
        nt.forget_glyphs();
    }
    layouts.cmap = image.colors.as_ref().map_or([0.0; 4], |c| {
        let (w, h) = (c.size.0.max(1) as f32, c.size.1.max(1) as f32);
        [c.at[0] as f32, c.at[1] as f32, 1.0 / w, 1.0 / h]
    });
    layouts.cells = std::mem::take(&mut image.cells);
    layouts.softer = image.scale < 1.0;
    layouts.atlas = image.size;
    Some(Laid {
        layouts,
        image: Some(image),
    })
}

/// The baked shadows of `layouts`' nodes that bake theirs: each layout's one cell of `image` blurred by
/// the node's Gaussian (`px` atlas pixels per 1080p pixel at the image's scale), within what the budget leaves and the
/// device's texture side; past either, those nodes' shadows are drawn offscreen as before.
fn bake_shadows(
    layouts: &mut TextLayouts,
    image: &mut AtlasImage,
    px: f32,
    (budget, max_side): (u64, u32),
) {
    let mut asks = Vec::new();
    for nt in &mut layouts.nodes {
        // A text whose box cuts its letters casts its shadow offscreen, from what is left of them.
        let cut = (0..nt.layouts.len()).any(|l| {
            let cell = image.cells.get(nt.cells[l] as usize);
            let clip = nt.layouts[l].clip && nt.layouts[l].pieces.len() == 1;
            cell.is_some_and(|c| !super::text_shadow::fits(c, nt.box_px, clip))
        });
        if cut {
            nt.shadow = None;
        }
        let Some(shadow) = &mut nt.shadow else {
            continue;
        };
        let sigma = shadow.sigma * px * image.scale;
        shadow.cells = (0..nt.layouts.len())
            .map(|l| match nt.layouts[l].pieces.len() {
                1 => {
                    asks.push((nt.cells[l] as usize, sigma));
                    (asks.len() - 1) as u32
                }
                _ => u32::MAX,
            })
            .collect();
    }
    let left = budget.saturating_sub(image.bytes());
    match super::text_shadow::bake(image, &asks, left, max_side) {
        Some((shadows, cells)) => {
            image.shadows = Some(shadows);
            layouts.shadows = cells;
        }
        None => layouts.nodes.iter_mut().for_each(|nt| nt.shadow = None),
    }
}
