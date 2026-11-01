//! A scene's text on the CPU alone (the engine's test builds, `test-support`): its instance built and its text laid out
//! as the build thread does it, then frames evaluated and packed by the drawer's own `Packed::pack`,
//! with no GPU. What the per-frame allocation and layout tests count, and what the caps and the atlas say.

use std::collections::BTreeMap;
use std::sync::Arc;

use klipolo_model::contrib::Catalog;
use klipolo_model::params::ParamValue;
use klipolo_model::scene::eval::{SceneFrame, eval_into};
use klipolo_model::scene::instance::SceneInstance;

use super::Drawn;
use super::meshes::Meshes;
use super::pack::{PackIn, Packed, Room};
use super::sizes::Sizes;
use super::statics::{NodeStatic, statics};
use super::text::{MAX_EM_PX, TextNotes};
use super::text_layouts::{TextLayouts, lay_out_all};
use crate::render::fonts::configure_for_tests;
use crate::render::shared_fonts::SharedFonts;

/// One scene instance's text, laid out once, drawn frame after frame into the same room.
#[doc(hidden)]
pub struct TextProbe {
    inst: SceneInstance,
    frame: SceneFrame,
    statics: Vec<NodeStatic>,
    spans: Vec<i32>,
    layouts: TextLayouts,
    atlas_bytes: u64,
    refused: bool,
    meshes: Meshes,
    packed: Packed,
    drawn: Drawn,
    backdrop: Option<[f32; 6]>,
    sizes: Sizes,
}

impl TextProbe {
    /// The scene `json` (checked as a stored scene is) with `params`, laid out for a target of `size` pixels, `px` of
    /// them per 1080p pixel, with Klipolo's own faces.
    pub fn new(
        json: &str,
        params: &BTreeMap<String, ParamValue>,
        size: (u32, u32),
        px: f32,
    ) -> Result<Self, String> {
        let scene = crate::registry::compile_scene(json.as_bytes(), Catalog::builtin())
            .map_err(|p| format!("{p:?}"))?;
        configure_for_tests();
        let mut fonts = SharedFonts::new(false);
        let mut inst = SceneInstance::new(&scene, params);
        let (statics, _, spans) = statics(&inst, &[]);
        let drawn = Drawn {
            size,
            k: f64::from(px),
        };
        let language: Arc<str> = Arc::from("en");
        let max_px = (size.0.max(size.1) as f32).min(MAX_EM_PX);
        let laid = lay_out_all(
            &mut fonts,
            &language,
            &mut inst,
            &statics,
            (drawn, max_px),
            8192,
        )
        .ok_or("no text")?;
        let frame = SceneFrame::for_instance(&inst);
        let meshes = Meshes::default();
        let mut packed = Packed::with_room(Room::of(&inst, &statics, &meshes));
        let room = (laid.layouts.most, laid.layouts.paints);
        packed.reserve_text(room, laid.layouts.nodes.len());
        Ok(Self {
            sizes: Sizes::with_room(inst.len()),
            atlas_bytes: laid.image.as_ref().map_or(0, |i| i.bytes()) + laid.layouts.bytes(),
            refused: laid.image.is_none(),
            layouts: laid.layouts,
            inst,
            frame,
            statics,
            spans,
            meshes,
            packed,
            drawn,
            backdrop: None,
        })
    }

    /// The scene `json` packed with no text laid out (shapes, offscreen groups, masks, particles, fills and,
    /// with `backdrop`, a backdrop mapped onto a frame under it), for the packing's allocation test.
    pub fn without_text(
        json: &str,
        size: (u32, u32),
        px: f32,
        backdrop: bool,
    ) -> Result<Self, String> {
        let scene = crate::registry::compile_scene(json.as_bytes(), Catalog::builtin())
            .map_err(|p| format!("{p:?}"))?;
        let inst = SceneInstance::new(&scene, &BTreeMap::new());
        let (statics, _, spans) = statics(&inst, &[]);
        let drawn = Drawn {
            size,
            k: f64::from(px),
        };
        let frame = SceneFrame::for_instance(&inst);
        let meshes = Meshes::default();
        let packed = Packed::with_room(Room::of(&inst, &statics, &meshes));
        Ok(Self {
            sizes: Sizes::with_room(inst.len()),
            atlas_bytes: 0,
            refused: false,
            layouts: TextLayouts::empty(drawn.k),
            inst,
            frame,
            statics,
            spans,
            meshes,
            packed,
            drawn,
            backdrop: backdrop.then_some([1.0 / 640.0, 0.0, 0.0, 1.0 / 360.0, 0.0, 0.0]),
        })
    }

    /// What the last frame drew offscreen, its generator fills and backdrops, and its shapes.
    pub fn drawn_parts(&self) -> (usize, usize, usize, usize) {
        let p = &self.packed;
        (
            p.isos.len(),
            p.fill_uses.len(),
            p.backdrop_uses.len(),
            p.shapes.len(),
        )
    }

    /// The frame at scene time `t` packed by the drawer's own packing: how many text draws it makes.
    pub fn frame(&mut self, t: f64) -> usize {
        eval_into(&self.inst, t, &mut self.frame);
        let f = PackIn {
            inst: &self.inst,
            frame: &self.frame,
            statics: &self.statics,
            spans: &self.spans,
            drawn: self.drawn,
            pictures: &[],
            meshes: &self.meshes,
            text: Some(&self.layouts),
            backdrop: self.backdrop,
        };
        self.packed.pack(&f);
        // Its offscreen nodes' targets placed, as the drawer places them.
        self.sizes.place(&mut self.packed.isos, self.drawn.size);
        self.packed.texts.len()
    }

    /// [`frame`](Self::frame) packed at `k` target pixels per 1080p pixel instead of the layout's own (a layer's motion
    /// scale under the drawer's old rule, or a stand-in laid out at another size): the text stays as laid out.
    pub fn frame_at_k(&mut self, t: f64, k: f64) -> usize {
        let kept = self.drawn.k;
        self.drawn.k = k;
        let n = self.frame(t);
        self.drawn.k = kept;
        n
    }

    /// The last frame's blurring pieces: each one's deviation and tap step in atlas texels,
    /// and the rows of the scratch target they are blurred across into.
    pub fn blurs(&self) -> (Vec<[f32; 2]>, u32) {
        let pieces = self.packed.texts.iter().filter(|d| d.misc[3] > 0.0);
        let step = |sigma: f32| super::text_blur::Kernel::of(sigma).step;
        let each = pieces.map(|d| [d.misc[3], step(d.misc[3])]).collect();
        (each, self.packed.blur.rows())
    }

    /// Where the last frame's first text draw's cell starts in the target, whatever moves it.
    pub fn first_text_corner(&self) -> Option<[f64; 2]> {
        let d = self.packed.texts.first()?;
        let [a, b, c, e] = d.m.map(f64::from);
        let (x, y) = (f64::from(d.rect[0]), f64::from(d.rect[1]));
        Some([
            a * x + c * y + f64::from(d.t[0]),
            b * x + e * y + f64::from(d.t[1]),
        ])
    }

    /// The last frame's shape `i` (in paint order, shadows counted): its box's left and top edges in the target.
    pub fn shape_corner(&self, i: usize) -> Option<[f64; 2]> {
        let s = self.packed.shapes.get(i)?;
        let m = s.at_half.map(f64::from);
        Some([m[0] - m[2], m[1] - m[3]])
    }

    /// The whole-pixel shift the last frame gave node `id`'s text (its rest pose's rounding).
    pub fn rest_shift(&self, id: &str) -> Option<[f64; 2]> {
        let c = self.inst.compiled().find(id)?;
        let i = self.inst.instances_of(c).next()?;
        let nt = self.layouts.node(i)?;
        let text = nt.layouts.first()?;
        let rest = self.packed.rest_world.get(i)?;
        Some(super::text_fit::rest_snap(nt, text, rest))
    }

    /// Where the last frame's unmoved draws (scaled and turned by nothing) start in the target: each cell's top left.
    pub fn rest_corners(&self) -> Vec<[f32; 2]> {
        self.packed
            .texts
            .iter()
            .filter(|d| d.m == [1.0, 0.0, 0.0, 1.0])
            .map(|d| [d.t[0] + d.rect[0], d.t[1] + d.rect[1]])
            .collect()
    }

    /// The units the instance placed node `id`'s tweens on (what the layout told it).
    pub fn units(&self, id: &str) -> usize {
        let Some(c) = self.inst.compiled().find(id) else {
            return 0;
        };
        self.inst.instances_of(c).map(|n| self.inst.units(n)).sum()
    }

    /// What its text tells the frames (a stand-in, CJK without its download, text cut or refused).
    pub fn notes(&self) -> TextNotes {
        self.layouts.notes
    }

    /// The characters laid out, every text of every node.
    pub fn chars_laid_out(&self) -> usize {
        let chars = |t: &super::text::TextUnits| t.text.chars().count();
        self.layouts
            .nodes
            .iter()
            .map(|n| {
                n.layouts.iter().map(chars).sum::<usize>()
                    + n.set.as_ref().map_or(0, |s| chars(s.units()))
            })
            .sum()
    }

    /// The bytes its atlas and layouts would hold (the GPU's and the CPU's).
    pub fn atlas_bytes(&self) -> u64 {
        self.atlas_bytes
    }

    /// Its atlas is drawn below full resolution for the budget.
    pub fn softer(&self) -> bool {
        self.layouts.softer
    }

    /// Its atlas was past the budget even at its softest: nothing kept, nothing drawn.
    pub fn refused(&self) -> bool {
        self.refused
    }
}
