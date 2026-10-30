//! How big each offscreen node's target is, and where it sits, and the scene's own: its box
//! (`bounds.rs`) in 16-pixel classes, the largest its motion needs. The first frame an instance opens an offscreen node
//! or reads a backdrop, its frames across its whole timeline are packed once (30 a second, at most 240: a survey, as
//! a backdrop's grab is sized for the largest box its layer's motion reaches), so a node that scales, turns,
//! slides or blurs more as it plays keeps one target size from its first frame and the pool hands the same target back
//! every frame; a frame that still needs more (a peak between two samples) grows its class once. A scene whose own
//! target is all that is sized is never surveyed on its frame: its survey is made on the text build
//! thread (with its text, or alone for a scene without: `sizes_built.rs`) and taken when it lands; until then (or for
//! one whose boxes need the render thread) its class grows as it plays, and once it outgrows its first class it takes
//! the layer's whole target (one target more, not one each time it grows). Classes are per target size; the survey runs
//! again when the instance's own text lands or its target changes size; never for a scene that holds one pose its
//! whole timeline (its first frame's boxes are its classes), nor while a stand-in text draws (an edit being built: its
//! own text surveys once it lands, so a keystroke surveys once). A target's top left follows its box on even pixels (a
//! blur that halves its target then averages the same pixels as over the scene's), moved inside the layer's target
//! rather than cut, so it keeps its size.

use std::time::Instant;

use klipolo_model::scene::eval::{SceneFrame, eval_into};
use klipolo_model::scene::instance::SceneInstance;

use super::Drawn;
use super::bounds::is_empty;
use super::groups::Iso;
use super::meshes::Meshes;
use super::pack::{PackIn, Packed};
use super::sizes_built::survey;
use super::statics::NodeStatic;
use super::text_layouts::TextLayouts;

/// A target's sides are whole multiples of this.
pub(super) const CLASS: u32 = 16;

/// Where a target sits in the layer's target: its top left and its size, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Placed {
    pub origin: [u32; 2],
    pub size: (u32, u32),
}

impl Placed {
    /// The whole of a layer target of `size`.
    pub(super) fn whole(size: (u32, u32)) -> Self {
        Self {
            origin: [0, 0],
            size,
        }
    }
}

/// `v` down to an even pixel.
fn even_floor(v: f32) -> f32 {
    (v / 2.0).floor() * 2.0
}

/// The class box `b` needs: what it spans from its even top left, in whole classes.
fn class_of(b: [f32; 4]) -> [u16; 2] {
    if is_empty(b) {
        return [CLASS as u16; 2];
    }
    let side = |lo: f32, hi: f32| {
        let n = (hi.ceil() - even_floor(lo)).clamp(1.0, f32::from(u16::MAX));
        ((n / CLASS as f32).ceil() as u32 * CLASS).min(u32::from(u16::MAX)) as u16
    };
    [side(b[0], b[2]), side(b[1], b[3])]
}

/// `c` grown to hold `b`'s class (classes only grow).
fn grown(c: &mut [u16; 2], b: [f32; 4]) {
    let want = class_of(b);
    *c = [c[0].max(want[0]), c[1].max(want[1])];
}

/// A target of class `c` for box `b` in a layer target of `size`: within it, its top left at the box's on even pixels,
/// moved inside rather than cut.
fn placed(c: [u16; 2], b: [f32; 4], size: (u32, u32)) -> Placed {
    let (w, h) = (
        u32::from(c[0]).clamp(1, size.0.max(1)),
        u32::from(c[1]).clamp(1, size.1.max(1)),
    );
    let at = |lo: f32, side: u32, of: u32| {
        let v = if is_empty(b) { 0.0 } else { even_floor(lo) };
        (v.max(0.0) as u32).min(of.saturating_sub(side))
    };
    Placed {
        origin: [at(b[0], w, size.0), at(b[1], h, size.1)],
        size: (w, h),
    }
}

/// One instance's classes: the largest each offscreen node's target has needed, by node, and the scene's own; the
/// largest box its backdrop nodes read (what the compositor grabs, once surveyed); the last survey's time.
#[derive(Default)]
pub(super) struct Sizes {
    per_node: Vec<[u16; 2]>,
    root: [u16; 2],
    backdrop: Option<[f32; 4]>,
    /// The target size and text (its entry's id) the last survey packed for.
    surveyed: Option<((u32, u32), u64)>,
    /// How long the last survey took (µs), and how many ran (the performance numbers).
    pub(super) survey_us: f64,
    pub(super) surveys: u64,
}

impl Sizes {
    /// Room for an instance of `nodes` nodes (made with it).
    pub(super) fn with_room(nodes: usize) -> Self {
        Self {
            per_node: vec![[0; 2]; nodes],
            ..Self::default()
        }
    }

    /// Classes for a target of another size (they are per size): none yet, nothing surveyed.
    pub(super) fn reset(&mut self) {
        self.per_node.iter_mut().for_each(|c| *c = [0; 2]);
        (self.root, self.backdrop, self.surveyed) = ([0; 2], None, None);
    }

    /// The target size the last survey packed for.
    pub(super) fn surveyed_for(&self) -> Option<(u32, u32)> {
        self.surveyed.map(|(s, _)| s)
    }

    /// A survey for a target of `size` with text `text` is what they hold.
    pub(super) fn mark_surveyed(&mut self, size: (u32, u32), text: u64) {
        self.surveyed = Some((size, text));
    }

    /// The CPU bytes they keep.
    pub(super) fn bytes(&self) -> usize {
        self.per_node.capacity() * std::mem::size_of::<[u16; 2]>()
    }

    /// The classes a survey on the text build thread found (`sizes_built.rs`), taken as this instance's own.
    pub(super) fn adopt(&mut self, built: &Sizes) {
        if built.per_node.len() == self.per_node.len() {
            self.per_node.copy_from_slice(&built.per_node);
            (self.root, self.backdrop, self.surveyed) =
                (built.root, built.backdrop, built.surveyed);
        }
    }

    /// Whether a survey found the classes for a target of `size` with text `text`.
    pub(super) fn surveyed(&self, size: (u32, u32), text: u64) -> bool {
        self.surveyed == Some((size, text))
    }

    /// What `p`'s offscreen nodes and the scene's own drawing need this frame, and what its backdrops read (classes and
    /// the backdrop's box only grow).
    pub(super) fn note(&mut self, p: &Packed) {
        for iso in &p.isos {
            if let Some(c) = self.per_node.get_mut(iso.node as usize) {
                grown(c, iso.bounds);
            }
        }
        grown(&mut self.root, p.root);
        self.note_backdrop(p.backdrop);
    }

    fn note_backdrop(&mut self, b: [f32; 4]) {
        if !is_empty(b) {
            let had = self.backdrop.unwrap_or(b);
            self.backdrop = Some([
                had[0].min(b[0]),
                had[1].min(b[1]),
                had[2].max(b[2]),
                had[3].max(b[3]),
            ]);
        }
    }

    /// The largest box its backdrop nodes read of a target of `size`, in whole 16-pixel classes from its even top left
    /// (x, y, w, h), once its timeline was surveyed for that size: None before (the compositor grabs the layer's whole
    /// box), or with no backdrop.
    pub(super) fn backdrop(&self, size: (u32, u32)) -> Option<[u32; 4]> {
        let b = self
            .backdrop
            .filter(|_| self.surveyed.is_some_and(|(s, _)| s == size))?;
        let p = placed(class_of(b), b, size);
        Some([p.origin[0], p.origin[1], p.size.0, p.size.1])
    }

    /// This frame's offscreen nodes placed in a layer target of `size`: each its node's class (grown to this frame's
    /// box), within the target, its top left at its box's on even pixels, moved inside the target rather than cut.
    pub(super) fn place(&mut self, isos: &mut [Iso], size: (u32, u32)) {
        for iso in isos {
            let c = match self.per_node.get_mut(iso.node as usize) {
                Some(c) => {
                    grown(c, iso.bounds);
                    *c
                }
                None => class_of(iso.bounds),
            };
            iso.target = placed(c, iso.bounds, size);
        }
    }

    /// The scene's own target this frame, for what it draws into it (`root`): its class (grown to this frame's box),
    /// placed as an offscreen node's is. Before its survey (`surveyed` false), a box that outgrows its
    /// first class takes the layer's whole target from then on.
    pub(super) fn place_root(
        &mut self,
        root: [f32; 4],
        size: (u32, u32),
        surveyed: bool,
    ) -> Placed {
        let first = self.root;
        grown(&mut self.root, root);
        if !surveyed && first != [0; 2] && self.root != first {
            self.root = class_of([0.0, 0.0, size.0 as f32, size.1 as f32]);
        }
        placed(self.root, root, size)
    }
}

/// What an instance's frames are packed with, but the frame: its pack's `PackIn` without it.
#[derive(Clone, Copy)]
pub(super) struct Parts<'a> {
    pub inst: &'a SceneInstance,
    pub statics: &'a [NodeStatic],
    pub spans: &'a [i32],
    pub drawn: Drawn,
    pub pictures: &'a [Option<(u32, u32)>],
    pub meshes: &'a Meshes,
    pub text: Option<&'a TextLayouts>,
    pub backdrop: Option<[f32; 6]>,
}

impl<'a> Parts<'a> {
    /// `frame` packed with these.
    pub(super) fn with(&self, frame: &'a SceneFrame) -> PackIn<'a> {
        PackIn {
            inst: self.inst,
            frame,
            statics: self.statics,
            spans: self.spans,
            drawn: self.drawn,
            pictures: self.pictures,
            meshes: self.meshes,
            text: self.text,
            backdrop: self.backdrop,
        }
    }
}

/// What a frame is packed for: its layer target's size and its text (its entry's id), whether the scene's own target
/// may be only its box (`shrink`: nothing reads the layer's whole target), whether its classes may be surveyed (its own
/// text, not a stand-in), and whether it holds one pose throughout (`still`: this frame is all a survey would find).
#[derive(Clone, Copy, Debug)]
pub(super) struct Packing {
    pub size: (u32, u32),
    pub text: u64,
    pub shrink: bool,
    pub survey: bool,
    pub still: bool,
}

/// Frame `t` (evaluated into `frame`) packed into `packed`, its offscreen nodes' targets sized and placed, and the
/// scene's own target: its box's when `p.shrink`, else the layer's whole target. The first frame one goes offscreen or
/// a backdrop is read, in a target of `p.size` with text `p.text`, the instance's timeline is surveyed first (when
/// `p.survey`), then this frame evaluated and packed again; a frame that sizes only the scene's own target waits for
/// the build thread's survey.
pub(super) fn pack(
    parts: &Parts,
    (frame, packed): (&mut SceneFrame, &mut Packed),
    sizes: &mut Sizes,
    t: f64,
    p: Packing,
) -> Placed {
    packed.pack(&parts.with(frame));
    let classes = !packed.isos.is_empty() || !packed.backdrop_uses.is_empty();
    if (classes || p.shrink) && p.survey && !sizes.surveyed(p.size, p.text) {
        if p.still {
            // One pose throughout: this frame's boxes are all a survey would find.
            sizes.surveyed = Some((p.size, p.text));
        } else if classes {
            let started = Instant::now();
            survey(parts, frame, packed, sizes, (p.size, p.text));
            sizes.survey_us = started.elapsed().as_secs_f64() * 1e6;
            sizes.surveys += 1;
            eval_into(parts.inst, t, frame);
            packed.pack(&parts.with(frame));
        }
    }
    sizes.place(&mut packed.isos, p.size);
    sizes.note_backdrop(packed.backdrop);
    if p.shrink {
        let surveyed = sizes.surveyed(p.size, p.text);
        sizes.place_root(packed.root, p.size, surveyed)
    } else {
        Placed::whole(p.size)
    }
}

#[cfg(test)]
#[path = "sizes_tests.rs"]
mod tests;
