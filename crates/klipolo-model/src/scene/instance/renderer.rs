//! What the renderer reads of an instance beyond its static fields: a node's rest pose with its repeat copy's gap
//! copy's gap (where its text is put on whole pixels), its repeat index, the
//! emitters, whether a backdrop draws, and a generator fill's settings as the node binds them. Never per frame beyond a
//! copy; allocation-free but for a fill's settings, read once with the instance.

use super::super::eval::NodeState;
use super::super::timeline::Prop;
use super::super::Val;
use super::SceneInstance;
use crate::params::ParamValue;

impl SceneInstance {
    /// Instance node `node`'s resting values with its repeat copy's gap added, as every frame adds it (`eval_into`):
    /// where it rests before any track moves it. A repeat's copies rest `index × gap` apart, so text measured here lands
    /// on whole pixels in every copy.
    pub fn rest_pose(&self, node: usize) -> Option<NodeState> {
        let n = self.nodes.get(node)?;
        let mut s = n.base;
        s.set(Prop::X, s.x + n.gap[0]);
        s.set(Prop::Y, s.y + n.gap[1]);
        s.set(Prop::XPx, s.x_px + n.gap[2]);
        s.set(Prop::YPx, s.y_px + n.gap[3]);
        Some(s)
    }

    /// `$i` of instance node `node`: its nearest repeat's instance index (0 outside a repeat).
    pub fn repeat_index(&self, node: usize) -> f64 {
        self.nodes.get(node).map_or(0.0, |n| n.index)
    }

    /// The emitters' instance nodes, in paint order.
    pub fn emitters(&self) -> &[usize] {
        &self.emitters
    }

    /// A backdrop node draws: the frame under the layer is read every frame, and no frame is ever still.
    pub fn has_backdrop(&self) -> bool {
        self.has_backdrop
    }

    /// A generator fill's setting as instance node `node` draws it: a `"$key"` bound to the scene's setting (a number,
    /// on/off, colour, choice or text), `"$i"` and `"$row.vN"` to its repeat's numbers and `"$row.label"` to its row's
    /// label; anything else as written (`"$$x"` is the text `$x`).
    pub fn fill_value(&self, node: usize, v: &ParamValue) -> ParamValue {
        let ParamValue::Text(s) = v else {
            return v.clone();
        };
        if super::super::check::binding(s).is_none() {
            return ParamValue::Text(super::super::literal(s).to_owned());
        }
        let ctx = self.ctx(node);
        let bound = Val::Str(s.clone());
        if let Some(n) = ctx.num(&bound) {
            return ParamValue::Number(n);
        }
        if let Some(b) = ctx.flag_val(&bound) {
            return ParamValue::Bool(b);
        }
        ParamValue::Text(ctx.text(s, None))
    }
}
