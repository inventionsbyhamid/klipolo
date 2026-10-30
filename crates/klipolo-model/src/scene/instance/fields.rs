//! The renderer's static fields of an instance node: texts, colours, numbers, lengths, points, images and
//! fonts, each read with the node's settings, repeat index and row. A number from a table row has no range the check
//! can hold it to (a table's values are unbounded), so every number and length is held here to its field's range
//! named by a [`Field`] so no misspelling can leave one unclamped.

use super::super::check::range_of;
use super::super::timeline::Track;
use super::super::units::{MAX_FRACTION, MAX_PX};
use super::super::{Points, SceneLimits, Val};
use super::{Ctx, SceneInstance};

/// A static number or length field the renderer reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// A star's points or a polygon's sides: 3–64, whole.
    Sides,
    /// A text's or a span's weight.
    Weight,
    /// A text's or a span's size, in 1080p pixels.
    Size,
    LineHeight,
    LetterSpacing,
    /// An outline's width, in 1080p pixels.
    OutlineWidth,
    /// A box's edges: x and y within ±10 boxes, width and height 0–10.
    BoxX,
    BoxY,
    BoxW,
    BoxH,
    /// A repeat's step between instances.
    GapX,
    GapY,
    /// The width a text wraps at.
    Wrap,
    /// A repeat's instances.
    Times,
    /// An emitter's burst and rate.
    Burst,
    Rate,
    /// A gradient's turn in degrees, and how far its stops slide.
    GradientAngle,
    GradientOffset,
}

impl Field {
    /// The values the field takes.
    pub fn range(self) -> (f64, f64) {
        let named = |name: &str| range_of(name).unwrap_or((-MAX_PX, MAX_PX));
        match self {
            Self::Sides => (3.0, 64.0),
            Self::Weight => named("weight"),
            Self::Size => named("size"),
            Self::LineHeight => named("lineHeight"),
            Self::LetterSpacing => named("letterSpacing"),
            Self::OutlineWidth => (0.0, 100.0),
            Self::BoxX | Self::BoxY | Self::GapX | Self::GapY => (-MAX_FRACTION, MAX_FRACTION),
            Self::BoxW | Self::BoxH => (0.0, MAX_FRACTION),
            Self::Wrap => named("wrap"),
            Self::Times => (0.0, SceneLimits::MAX_INSTANCES as f64),
            Self::Burst | Self::Rate => (0.0, 1e6),
            Self::GradientAngle => named("fill.angle"),
            Self::GradientOffset => named("fill.offset"),
        }
    }
}

impl SceneInstance {
    /// A text field of instance node `node` as it shows (a span's text, a bound label), its case not applied.
    pub fn text_of(&self, node: usize, s: &str) -> String {
        self.ctx(node).text(s, None)
    }

    /// A colour field of `node` (a span's fill, an outline, a gradient stop), bound or literal.
    pub fn color_of(&self, node: usize, s: &str) -> Option<[f32; 4]> {
        self.ctx(node).color(s)
    }

    /// The number field `field` of `node`, bound or literal, held to the field's range (`Sides` rounded to a whole
    /// number).
    pub fn number_of(&self, node: usize, field: Field, v: &Val) -> Option<f64> {
        let n = self.ctx(node).num(v)?;
        let (lo, hi) = field.range();
        let n = n.clamp(lo, hi);
        Some(if field == Field::Sides { n.round() } else { n })
    }

    /// The length field `field` of `node` (a box edge, a gap, the wrap width) as a fraction of the box and pixels, each
    /// held to its range.
    pub fn length_of(&self, node: usize, field: Field, v: &Val) -> Option<[f64; 2]> {
        let [f, px] = self.ctx(node).len(v, false)?;
        let (lo, hi) = field.range();
        Some([f.clamp(lo, hi), px.clamp(-MAX_PX, MAX_PX)])
    }

    /// A polyline's points, fractions of its box: as written (within ±10 boxes), or a table setting's rows as a chart
    /// row `i` of `n` at `x = i / (n − 1)`, its first number `v` at `y = 1 − (v − min) / (max − min)`, so
    /// the line rises with its values (level in the middle when they are all equal).
    pub fn points_of(&self, node: usize, points: &Points) -> Vec<[f64; 2]> {
        match points {
            Points::Bind(s) => {
                let rows = self.ctx(node).rows(s).unwrap_or_default();
                let values: Vec<f64> = rows
                    .iter()
                    .map(|r| {
                        r.values
                            .first()
                            .copied()
                            .filter(|v| v.is_finite())
                            .unwrap_or(0.0)
                    })
                    .collect();
                let (lo, hi) = values
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
                let n = values.len();
                values
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        let x = if n > 1 {
                            i as f64 / (n - 1) as f64
                        } else {
                            0.5
                        };
                        let y = if hi > lo {
                            1.0 - (v - lo) / (hi - lo)
                        } else {
                            0.5
                        };
                        debug_assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
                        [x, y]
                    })
                    .collect()
            }
            Points::List(list) => {
                let held = |v: f64| {
                    if v.is_finite() {
                        v.clamp(-MAX_FRACTION, MAX_FRACTION)
                    } else {
                        0.0
                    }
                };
                list.iter().map(|&[x, y]| [held(x), held(y)]).collect()
            }
        }
    }

    /// Instance node `node`'s motion path, flattened when the instance was built.
    pub fn motion_table(&self, node: usize) -> Option<&super::super::eval::MotionTable> {
        self.motions.get(self.nodes.get(node)?.cnode)?.as_ref()
    }

    /// Instance node `node`'s resting values (its settings resolved), before any track moves them.
    pub fn rest(&self, node: usize) -> Option<&super::super::eval::NodeState> {
        Some(&self.nodes.get(node)?.base)
    }

    /// Whether anything moves one of `node`'s properties that `which` picks (a key, a tween or a modifier).
    pub fn animates(
        &self,
        node: usize,
        which: impl Fn(super::super::timeline::Prop) -> bool,
    ) -> bool {
        let Some(n) = self.nodes.get(node) else {
            return false;
        };
        let tracks = self.lists.get(n.list).into_iter().flatten();
        let mods = self.mods.get(n.cnode).into_iter().flatten();
        tracks
            .filter_map(|&i| self.pool.get(i as usize).map(|t| t.prop))
            .chain(mods.map(|(t, _)| t.prop))
            .any(which)
    }

    /// The image id an `"$image"` field of `node` names.
    pub fn asset_of(&self, node: usize, s: &str) -> Option<&str> {
        self.ctx(node).asset(s)
    }

    /// The face a `"$font"` field of `node` names.
    pub fn font_of(&self, node: usize, s: &str) -> Option<&crate::font::FontRef> {
        self.ctx(node).font(s)
    }

    /// The instance node `node` sits in (its box is a fraction of that one's): None at the top.
    pub fn parent(&self, node: usize) -> Option<usize> {
        self.nodes.get(node)?.parent
    }

    /// The latest moment before any of its exits starts, else the scene's end: where its still is drawn (the hold
    /// hold point).
    pub fn hold_time(&self) -> f64 {
        super::hold::hold_point(self)
    }

    /// About how many bytes the instance holds on the heap (its nodes and their texts, its tracks and their keys, its
    /// lists and its still): for the renderer's memory bound. Allocates nothing.
    pub fn approx_bytes(&self) -> usize {
        use std::mem::size_of;
        let texts: usize = self
            .nodes
            .iter()
            .map(|n| n.texts.iter().map(String::capacity).sum::<usize>() + n.texts.capacity() * 24)
            .sum();
        let keys: usize = self
            .pool
            .iter()
            .map(|t| t.keys.capacity() * size_of::<crate::motion::Key>())
            .sum();
        let lists: usize = self.lists.iter().map(|l| l.capacity() * 4).sum();
        let per_tween: usize = self
            .targets
            .iter()
            .zip(&self.bases)
            .map(|(t, b)| t.capacity() * 8 + b.capacity() * 4)
            .sum();
        self.nodes.capacity() * size_of::<super::INode>()
            + texts
            + self.pool.capacity() * size_of::<Track>()
            + keys
            + lists
            + per_tween
            + self.still.as_ref().map_or(0, |s| s.approx_bytes())
    }
}

/// A node's view of the settings.
pub(super) fn ctx_of(inst: &SceneInstance, node: usize) -> Ctx<'_> {
    let n = inst.nodes.get(node);
    Ctx {
        vals: &inst.values,
        index: n.map_or(0.0, |n| n.index),
        row: n.and_then(|n| n.row.as_deref()),
    }
}
