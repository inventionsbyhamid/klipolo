//! One node of a frame as draws (a node is drawn from three places: in
//! paint order, at each particle of its emitter, and in a mask's segment): its SDF shape (a generator fill's shape with
//! the generator's target bound: `fills.rs`), its path draws, its text, its image or its backdrop, after the draws
//! before it. Allocation-free: the frame's lists keep their room.

use klipolo_model::scene::eval::NodeState;

use super::backdrop;
use super::draws;
use super::fills;
use super::pack::{PackIn, Packed, Run};
use super::place::World;
use super::shape_pack::push_shape;
use super::statics::{Look, NodeStatic};
use super::text_pack::{NodeFrame, TextOut, pack_node};

impl Packed {
    /// The shapes run so far, closed before a draw of another kind.
    pub(super) fn close_shapes(&mut self) {
        let end = self.shapes.len() as u32;
        if end > self.run_start {
            self.runs.push(Run::Shapes(self.run_start, end));
        }
        self.run_start = end;
    }

    /// Node `i` (`s`, static part `st`) drawn at `here` (its text's whole-pixel shift measured at `rest_here`).
    pub(super) fn node(
        &mut self,
        f: &PackIn,
        i: usize,
        here: &World,
        rest_here: &World,
        s: &NodeState,
        st: &NodeStatic,
    ) {
        let k = f.drawn.k;
        let path = f.meshes.get(i);
        if path.is_some() && (st.outline || matches!(st.look, Look::Path | Look::Polyline)) {
            // The shape's fill (a trimmed or dashed shape's; its stroke is the path's), then its path draws. A line
            // has no fill.
            if st.outline && !matches!(st.look, Look::Line { .. }) {
                self.shape(i, here, s, st, st.look, k);
            }
            self.close_shapes();
            // A path filled by a generator draws its stroke alone (generator fills are SDF shapes': `fills.rs`).
            let unfilled = NodeState {
                fill: [0.0; 4],
                ..*s
            };
            let s = if st.fill == -1 { s } else { &unfilled };
            if let Some(nm) = path {
                draws::push(&mut self.draws, &mut self.runs, i, here, s, st, nm, k);
            }
            return;
        }
        match st.look {
            Look::None | Look::Path | Look::Polyline => {}
            Look::Text | Look::Emoji => self.text(f, i, here, rest_here, s, st),
            Look::Image(picture) => {
                let Some(&Some(dims)) = f.pictures.get(usize::from(picture)) else {
                    return;
                };
                self.close_shapes();
                let from = self.shapes.len() as u32;
                // Its shadow, if any, before it in its run (a shadow samples no picture).
                push_shape(&mut self.shapes, here, s, st, st.look, k, Some(dims));
                let to = self.shapes.len() as u32;
                match self.runs.last_mut() {
                    // Particles of one picture share a run.
                    Some(Run::Image {
                        to: last,
                        picture: p,
                        ..
                    }) if *last == from && *p == picture => *last = to,
                    _ => self.runs.push(Run::Image { from, to, picture }),
                }
                self.run_start = to;
            }
            Look::Backdrop => backdrop::push(self, here, s, st, f.backdrop, k),
            look => self.shape(i, here, s, st, look, k),
        }
    }

    /// Node `i`'s SDF shape (`look`): filled by its generator's target when it has one that draws (`fills.rs`), its
    /// stroke and shadow alone when its generator isn't drawn here (not checked yet, or not installed).
    fn shape(
        &mut self,
        i: usize,
        here: &World,
        s: &NodeState,
        st: &NodeStatic,
        look: Look,
        k: f64,
    ) {
        if st.fill >= 0 && fills::fillable(look) {
            fills::push(self, i, here, s, st, look, k);
        } else if st.fill == fills::NOT_DRAWN {
            let unfilled = NodeState {
                fill: [0.0; 4],
                ..*s
            };
            push_shape(&mut self.shapes, here, &unfilled, st, look, k, None);
        } else {
            push_shape(&mut self.shapes, here, s, st, look, k, None);
        }
    }

    /// A text node's (or an emoji's) pieces, in their own run (joined to the one before when it ends where they start:
    /// particles).
    fn text(
        &mut self,
        f: &PackIn,
        i: usize,
        here: &World,
        rest_here: &World,
        s: &NodeState,
        st: &NodeStatic,
    ) {
        let (Some(entry), Some(rest)) = (f.text, f.inst.rest(i)) else {
            return;
        };
        let from = self.texts.len() as u32;
        let nf = NodeFrame {
            node: i,
            here,
            s,
            rest,
            st,
            units: f.frame.units(i),
            buffer: f.frame.text(f.inst, i),
            rest_here,
            k: entry.k,
            spans: f.spans,
            blur: &self.blur,
            atlas: entry.atlas,
            cmap: entry.cmap,
        };
        let mut out = TextOut {
            draws: &mut self.texts,
            paints: &mut self.text_paints,
            room: (entry.most, entry.paints),
        };
        pack_node(&mut out, entry, i, &nf);
        let to = self.texts.len() as u32;
        if to == from {
            return;
        }
        self.close_shapes();
        match self.runs.last_mut() {
            Some(Run::Text(_, end)) if *end == from => *end = to,
            _ => self.runs.push(Run::Text(from, to)),
        }
    }
}
