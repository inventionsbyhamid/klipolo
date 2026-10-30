//! A frame of a scene instance as draws for the GPU: every drawn node placed
//! (`place.rs`) and drawn in paint order (`pack_node.rs`): SDF shapes in runs, images with their pictures bound, path
//! draws, text runs, generator-filled shapes and backdrops with their targets bound. A
//! node drawn offscreen (`groups.rs`) opens a `Begin` and its subtree's end a matching `End`; an emitter's particles
//! draw where its particle shape sits (`emitter.rs`); each mask an offscreen group uses gets its own segment after the
//! frame's runs (`masks.rs`). Allocation-free once warm: every list keeps the room made with the instance (`Room`; the
//! text's when its text is built, `reserve_text`).

use klipolo_model::scene::eval::SceneFrame;
use klipolo_model::scene::instance::SceneInstance;

use super::Drawn;
use super::backdrop::BackdropUse;
use super::bounds;
use super::emitter;
use super::fills::FillUse;
use super::groups::{self, Counts, Iso, MAX_ISOS};
use super::masks;
use super::meshes::Meshes;
use super::motion::motion_at;
use super::path_pipeline::PathDraw;
use super::place::{GpuClip, IDENTITY, World, place};
use super::shapes::ShapeInstance;
use super::statics::NodeStatic;
use super::text_layouts::TextLayouts;
use super::text_pack::BlurBudget;
use super::text_pipeline::{TextDraw, TextPaintDraw};

pub(super) use super::pack_room::Room;

/// What a frame draws, in paint order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Run {
    /// Shapes `[start, end)` of the frame's list.
    Shapes(u32, u32),
    /// Image shapes `[from, to)` (their shadows among them), with the layer's picture `picture` (`SceneLayer::images`).
    Image {
        from: u32,
        to: u32,
        picture: u16,
    },
    /// Node `node`'s path mesh, its fill or its stroke, drawn by path draw `draw`; `once`, a translucent
    /// stroke blending once a pixel through the stencil.
    Path {
        node: u32,
        draw: u32,
        stroke: bool,
        once: bool,
    },
    /// Text draws `[start, end)` of the frame's text list.
    Text(u32, u32),
    /// Shapes `[from, to)` filled by the frame's generator fill `fill`.
    Fill {
        from: u32,
        to: u32,
        fill: u16,
    },
    /// Shapes `[from, to)` filled by the frame's backdrop copy `slot`.
    Backdrop {
        from: u32,
        to: u32,
        slot: u16,
    },
    /// Offscreen node `isos[slot]` begins: what follows draws into a target of its own, until its `End`.
    Begin(u8),
    End(u8),
    /// Offscreen node `isos[slot]`'s mask begins (after the frame's own runs), until its `MaskEnd`.
    Mask(u8),
    MaskEnd(u8),
}

/// What a frame's pack reads: the instance, its frame and static part, the spans' gradients, where it is drawn, the
/// layer's pictures that are ready, its path meshes, its text, and how its target maps onto the frame under the layer
/// (backdrops) when that was grabbed.
#[derive(Clone, Copy)]
pub(super) struct PackIn<'a> {
    pub inst: &'a SceneInstance,
    pub frame: &'a SceneFrame,
    pub statics: &'a [NodeStatic],
    pub spans: &'a [i32],
    pub drawn: Drawn,
    pub pictures: &'a [Option<(u32, u32)>],
    pub meshes: &'a Meshes,
    pub text: Option<&'a TextLayouts>,
    pub backdrop: Option<[f32; 6]>,
}

/// A frame's draws in paint order, the runs they draw in, every node's place, what goes offscreen and what fills and
/// backdrops it draws: kept between frames (their room stays, so packing allocates nothing once warm).
#[derive(Default)]
pub(super) struct Packed {
    pub world: Vec<World>,
    /// Every node's place at its rest pose, nothing moving (what text is put on whole pixels at),
    /// kept for an instance with text.
    pub rest_world: Vec<World>,
    pub shapes: Vec<ShapeInstance>,
    pub runs: Vec<Run>,
    pub draws: Vec<PathDraw>,
    pub texts: Vec<TextDraw>,
    /// What the text draws name: each node's pieces of one look share one.
    pub text_paints: Vec<TextPaintDraw>,
    pub isos: Vec<Iso>,
    /// What the scene draws into its own target this frame (`bounds.rs`: its pixels, x0, y0, x1, y1; empty when
    /// nothing), and what its backdrop nodes read of the frame under it.
    pub root: [f32; 4],
    pub backdrop: [f32; 4],
    /// What `rest_world` was placed for, when an instance's text needed it: its target's size and the text's layout
    /// scale (a rest pose doesn't move, so it is placed once, not every frame).
    pub(super) rest_key: Option<((u32, u32), u64)>,
    /// Open offscreen nodes: where each one's subtree ends, and its slot.
    pub(super) open: Vec<(u32, u8)>,
    /// Where the masks' segments start in `runs`.
    pub mask_from: usize,
    pub fill_uses: Vec<FillUse>,
    pub backdrop_uses: Vec<BackdropUse>,
    pub counts: Counts,
    /// Something past a runtime cap on fills, backdrops, emitters or blurring units was left out.
    pub reduced: bool,
    pub emitters_drawn: usize,
    pub blur: BlurBudget,
    /// Where the shapes run being made starts.
    pub run_start: u32,
    /// The runs the shapes and paths alone take at most.
    pub(super) runs_room: usize,
}

impl Packed {
    /// The frame `f` packed: every node placed and drawn in paint order, offscreen nodes opened and closed around their
    /// subtrees, particles at their shapes, and the masks' segments last.
    pub(super) fn pack(&mut self, f: &PackIn) {
        let Drawn { size, k } = f.drawn;
        // Its rest poses, for text only (a scene without text spends nothing on them), kept while they hold.
        let rest_key = f.text.map(|t| (size, t.k.to_bits()));
        let rest_kept = rest_key.is_some() && self.rest_key == rest_key;
        self.world.clear();
        if !rest_kept {
            self.rest_world.clear();
        }
        self.shapes.clear();
        self.runs.clear();
        self.draws.clear();
        self.texts.clear();
        self.text_paints.clear();
        self.isos.clear();
        self.open.clear();
        self.fill_uses.clear();
        self.backdrop_uses.clear();
        self.counts = Counts::default();
        self.reduced = false;
        self.emitters_drawn = 0;
        self.blur.reset();
        self.run_start = 0;
        let root = World {
            m: IDENTITY,
            w: f64::from(size.0),
            h: f64::from(size.1),
            opacity: 1.0,
            clips: [GpuClip::default(); 2],
        };
        let (inst, statics) = (f.inst, f.statics);
        for (i, s) in f.frame.nodes().iter().enumerate() {
            self.close_ended(i);
            let parent = inst
                .parent(i)
                .and_then(|p| self.world.get(p))
                .copied()
                .unwrap_or(root);
            let rest_parent = inst
                .parent(i)
                .and_then(|p| self.rest_world.get(p))
                .copied()
                .unwrap_or(root);
            let Some(st) = statics.get(i) else {
                self.world.push(parent);
                if !rest_kept {
                    self.rest_world.push(rest_parent);
                }
                continue;
            };
            let at = |s, parent: &World, k: f64| {
                let motion = st
                    .motion
                    .and_then(|m| motion_at(inst, i, s, &m, (parent.w, parent.h), k));
                place(parent, s, st, k, motion)
            };
            let mut here = at(s, &parent, k);
            let shows = s.drawn && here.opacity > 0.0;
            // A text whose shadow is baked draws it in its own pass.
            let baked = f
                .text
                .and_then(|t| t.node(i))
                .is_some_and(|nt| nt.shadow.is_some());
            if shows
                && !st.particle
                && let Some(group) = groups::wanted(s, st, &parent, &here, baked)
                && self.open.len() < MAX_ISOS
                && self.counts.take(group)
            {
                let iso = groups::open(i, (s, st, baked), &parent, &mut here, k);
                let slot = self.isos.len() as u8;
                self.isos.push(iso);
                self.close_shapes();
                self.runs.push(Run::Begin(slot));
                self.open.push((st.end, slot));
            }
            self.world.push(here);
            // Its rest pose too, for text only: its repeat copy's gap included, placed at the text's own
            // layout scale, the same every frame of a layer's scaling motion and for a stand-in of another size.
            let rest_place = match (rest_kept, f.text, inst.rest_pose(i)) {
                (true, _, _) => self.rest_world.get(i).copied().unwrap_or(here),
                (false, Some(t), Some(r)) => at(&r, &rest_parent, t.k),
                _ => here,
            };
            if !rest_kept {
                self.rest_world.push(rest_place);
            }
            if !shows {
                continue;
            }
            if st.particle {
                emitter::particles(self, f, i, s, st);
                continue;
            }
            self.node(f, i, &here, &rest_place, s, st);
        }
        self.rest_key = rest_key;
        self.close_ended(usize::MAX);
        self.close_shapes();
        masks::segments(self, f);
        self.reduced |= self.blur.over();
        // Each offscreen node's own box, for a target only that big, and the scene's own.
        bounds::of_frame(self, f.meshes, size, k);
    }

    /// Closes every open offscreen node whose subtree ends before node `i`.
    fn close_ended(&mut self, i: usize) {
        while let Some(&(end, slot)) = self.open.last() {
            if (i as u64) < u64::from(end) {
                break;
            }
            self.close_shapes();
            self.runs.push(Run::End(slot));
            self.open.pop();
        }
    }
}
