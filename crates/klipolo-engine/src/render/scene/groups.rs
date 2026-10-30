//! Which nodes a frame draws offscreen, and how each is put back. A group goes offscreen only for blur, a
//! blend other than normal, a mask, a 2.5D tilt (`rotateX`/`rotateY`) or opacity below 1 (the model's `isolated`);
//! another node only when it can't be drawn right in place: a blur or a blend of its own, a shadow that isn't a rect's,
//! an ellipse's or an image's (text's among them: CSS's, of the whole silhouette, once), a third clip around it (two
//! clips a draw), or a fill under a stroke drawn apart at fractional opacity (it would show through the stroke).
//!
//! Inside, the node and its children draw at full opacity and unclipped into a pooled target of its own box
//! `bounds.rs`, `sizes.rs`; `SceneBytes` counts it at its size); its effects run in CSS's order (blur, its shadow under
//! it, its mask) and it is laid back with its opacity, blend, clips and tilt (`composite.rs`), or as a quad
//! (`quad_lay.rs`). At most [`MAX_GROUPS`] groups and
//! [`MAX_OTHERS`] other nodes a frame, past which one draws in place without its effect and the frame says
//! `frame.sceneTooManyGroups` once per scene; nesting is bounded by the same counts. Deciding is arithmetic on the
//! frame's values: nothing allocates.

use klipolo_model::motion::Blend;
use klipolo_model::scene::NodeType;
use klipolo_model::scene::eval::{ClipShape, NodeState};

use super::bounds::{EMPTY, NO_PARENT};
use super::composite::{Lay, Tilt, mode};
use super::place::{GpuClip, World};
use super::sizes::Placed;
use super::statics::{Look, NodeStatic};

/// The most groups drawn offscreen in a frame (the check's `MAX_GROUPS_OFFSCREEN`).
pub const MAX_GROUPS: usize = klipolo_model::scene::SceneLimits::MAX_GROUPS_OFFSCREEN;
/// The most other nodes drawn offscreen in a frame (a blurred shape, a text's shadow, a third clip…).
pub const MAX_OTHERS: usize = 4;
/// Every offscreen node a frame can open.
pub(super) const MAX_ISOS: usize = MAX_GROUPS + MAX_OTHERS;

/// A node's mask: which instance node it shows through, by its alpha or (`luma`) its brightness, or the reverse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MaskOf {
    pub node: u32,
    pub luma: bool,
    pub invert: bool,
}

/// A shadow drawn under an offscreen node: its offset in target pixels, its Gaussian in 1080p pixels, its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct IsoShadow {
    pub offset: [f32; 2],
    pub sigma: f64,
    pub color: [f32; 4],
}

/// One node drawn offscreen this frame: how it is laid back, its blur (a Gaussian in 1080p pixels), its mask and its
/// shadow; its own box this frame and the offscreen node it is laid back into (`bounds.rs`), and where its target sits
/// (`sizes.rs`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Iso {
    pub node: u32,
    pub lay: Lay,
    pub blur: f64,
    pub mask: Option<MaskOf>,
    pub shadow: Option<IsoShadow>,
    pub bounds: [f32; 4],
    pub parent: u8,
    pub target: Placed,
}

impl Iso {
    /// Whether it is laid back as a quad (`quad_lay.rs`): a normal blend, no tilt (a mask is applied before, at its own
    /// box). Else the host pass.
    pub(super) fn quad(&self) -> bool {
        self.lay.mode == 0.0 && self.lay.tilt.is_none()
    }
}

/// Whether a node draws offscreen this frame: `Some(true)` a group for its own effects (the model's `isolated`),
/// `Some(false)` any node that can't be drawn right in place (a group with a third clip among them), else None. A text
/// whose shadow is `baked` draws it in the text pass.
pub(super) fn wanted(
    s: &NodeState,
    st: &NodeStatic,
    parent: &World,
    here: &World,
    baked: bool,
) -> Option<bool> {
    if st.kind == NodeType::Group && s.isolated {
        return Some(true);
    }
    let third_clip = s.clip.shape != ClipShape::None && parent.clips[1].t[2] > 0.5;
    if third_clip {
        return Some(false);
    }
    // A text blurs its own units (`text_blur.rs`); a backdrop's blur is what it shows of the frame under it.
    let blur = s.blur > 0.0
        && !matches!(
            st.look,
            Look::Text | Look::Emoji | Look::None | Look::Backdrop
        );
    let shadow = shadow_of(s, st).is_some() && !baked;
    let split_stroke = here.opacity < 1.0
        && st.split_stroke
        && s.fill[3] > 0.0
        && s.stroke[3] > 0.0
        && s.stroke_width > 0.0;
    let own = st.blend != Blend::Normal || blur || shadow || split_stroke;
    (own && st.look != Look::None).then_some(false)
}

/// A node's shadow drawn by the group pass: any but a rect's, an ellipse's or an image's (those are analytic, in the
/// shapes pipeline), when it shows.
fn shadow_of(s: &NodeState, st: &NodeStatic) -> Option<()> {
    let sh = s.shadow;
    let shows = sh.color[3] > 0.0 && (sh.blur > 0.0 || sh.x != 0.0 || sh.y != 0.0);
    let analytic = matches!(st.look, Look::Rect | Look::Ellipse | Look::Image(_));
    (shows && !analytic && st.look != Look::None).then_some(())
}

/// Node `i` (`s`, static part `st`, unisolated place `here` in `parent`) drawn offscreen: how it is put back, and its
/// place inside, at full opacity and unclipped (`here` is made so). `k` target pixels per 1080p pixel. A text whose
/// shadow is `baked` casts it from the text pass, not from here.
pub(super) fn open(
    i: usize,
    (s, st, baked): (&NodeState, &NodeStatic, bool),
    parent: &World,
    here: &mut World,
    k: f64,
) -> Iso {
    let own = (s.clip.shape != ClipShape::None).then_some(here.clips[0]);
    let none = GpuClip::default();
    let clips = match own {
        Some(c) => [c, parent.clips[0], parent.clips[1]],
        None => [parent.clips[0], parent.clips[1], none],
    };
    let origin = {
        let (ox, oy) = (
            s.origin[0] * here.w + s.origin_px[0] * k,
            s.origin[1] * here.h + s.origin_px[1] * k,
        );
        let m = here.m;
        [m[0] * ox + m[2] * oy + m[4], m[1] * ox + m[3] * oy + m[5]]
    };
    let tilt = (st.kind == NodeType::Group)
        .then(|| Tilt::of(origin, s.rotate_x, s.rotate_y, s.perspective * k))
        .flatten();
    let shadow = shadow_of(s, st).filter(|_| !baked).map(|()| {
        let sh = s.shadow;
        let (x, y) = (sh.x * k, sh.y * k);
        let m = here.m;
        IsoShadow {
            offset: [(m[0] * x + m[2] * y) as f32, (m[1] * x + m[3] * y) as f32],
            // CSS's shadow blur is twice its deviation.
            sigma: sh.blur.max(0.0) / 2.0,
            color: sh.color,
        }
    });
    let iso = Iso {
        node: i as u32,
        lay: Lay {
            mode: mode(st.blend),
            opacity: here.opacity.clamp(0.0, 1.0) as f32,
            offset: [0.0; 2],
            tilt,
            clips,
            own_clip: own.is_some(),
            ..Lay::default()
        },
        blur: if matches!(st.look, Look::Text | Look::Emoji | Look::Backdrop) {
            0.0
        } else {
            s.blur.max(0.0)
        },
        mask: st.mask,
        shadow,
        bounds: EMPTY,
        parent: NO_PARENT,
        target: Placed::default(),
    };
    here.opacity = 1.0;
    here.clips = [none; 2];
    iso
}

/// What a frame opened offscreen so far: its groups and other nodes, and whether one had to draw in place.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Counts {
    pub groups: usize,
    pub others: usize,
    pub too_many: bool,
}

impl Counts {
    /// Whether one more (a group, or another node) may open: false past the caps, which the frame then says.
    pub(super) fn take(&mut self, group: bool) -> bool {
        let (n, cap) = if group {
            (&mut self.groups, MAX_GROUPS)
        } else {
            (&mut self.others, MAX_OTHERS)
        };
        if *n >= cap {
            self.too_many = true;
            return false;
        }
        *n += 1;
        true
    }
}
