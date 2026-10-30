//! Reading and writing a node's or a unit's state by property, each write held within the property's range.

use super::super::timeline::Prop;
use super::state::{NodeState, UnitState};

impl NodeState {
    /// The value `p` has (a clip or text index reads as 0).
    pub fn get(&self, p: Prop) -> f64 {
        use Prop as P;
        match p {
            P::X => self.x,
            P::XPx => self.x_px,
            P::Y => self.y,
            P::YPx => self.y_px,
            P::OriginX => self.origin[0],
            P::OriginXPx => self.origin_px[0],
            P::OriginY => self.origin[1],
            P::OriginYPx => self.origin_px[1],
            P::Scale => self.scale,
            P::ScaleX => self.scale_x,
            P::ScaleY => self.scale_y,
            P::Rotate => self.rotate,
            P::RotateX => self.rotate_x,
            P::RotateY => self.rotate_y,
            P::Perspective => self.perspective,
            P::Opacity => self.opacity,
            P::Visible => f64::from(u8::from(self.visible)),
            P::Blur => self.blur,
            P::Clip | P::Text => 0.0,
            P::FillR => f64::from(self.fill[0]),
            P::FillG => f64::from(self.fill[1]),
            P::FillB => f64::from(self.fill[2]),
            P::FillA => f64::from(self.fill[3]),
            P::FillOffset => self.fill_offset,
            P::FillAngle => self.fill_angle,
            P::StrokeR => f64::from(self.stroke[0]),
            P::StrokeG => f64::from(self.stroke[1]),
            P::StrokeB => f64::from(self.stroke[2]),
            P::StrokeA => f64::from(self.stroke[3]),
            P::StrokeWidth => self.stroke_width,
            P::DashOffset => self.dash_offset,
            P::TrimStart => self.trim[0],
            P::TrimEnd => self.trim[1],
            P::TrimOffset => self.trim[2],
            P::Radius => self.radius[0],
            P::InnerRadius => self.inner_radius,
            P::ShadowX => self.shadow.x,
            P::ShadowY => self.shadow.y,
            P::ShadowBlur => self.shadow.blur,
            P::ShadowSpread => self.shadow.spread,
            P::ShadowR => f64::from(self.shadow.color[0]),
            P::ShadowG => f64::from(self.shadow.color[1]),
            P::ShadowB => f64::from(self.shadow.color[2]),
            P::ShadowA => f64::from(self.shadow.color[3]),
            P::Progress => self.progress,
            P::Size => self.size,
            P::LetterSpacing => self.letter_spacing,
            P::LineHeight => self.line_height,
            P::TintR => f64::from(self.tint[0]),
            P::TintG => f64::from(self.tint[1]),
            P::TintB => f64::from(self.tint[2]),
            P::TintA => f64::from(self.tint[3]),
        }
    }

    /// Sets `p` to `v`, held within its range (`radius` sets every corner; a clip or text index is set elsewhere).
    pub fn set(&mut self, p: Prop, v: f64) {
        use Prop as P;
        let v = p.clamp(v);
        let c = v as f32;
        match p {
            P::X => self.x = v,
            P::XPx => self.x_px = v,
            P::Y => self.y = v,
            P::YPx => self.y_px = v,
            P::OriginX => self.origin[0] = v,
            P::OriginXPx => self.origin_px[0] = v,
            P::OriginY => self.origin[1] = v,
            P::OriginYPx => self.origin_px[1] = v,
            P::Scale => self.scale = v,
            P::ScaleX => self.scale_x = v,
            P::ScaleY => self.scale_y = v,
            P::Rotate => self.rotate = v,
            P::RotateX => self.rotate_x = v,
            P::RotateY => self.rotate_y = v,
            P::Perspective => self.perspective = v,
            P::Opacity => self.opacity = v,
            P::Visible => self.visible = v >= 0.5,
            P::Blur => self.blur = v,
            P::Clip | P::Text => {}
            P::FillR => self.fill[0] = c,
            P::FillG => self.fill[1] = c,
            P::FillB => self.fill[2] = c,
            P::FillA => self.fill[3] = c,
            P::FillOffset => self.fill_offset = v,
            P::FillAngle => self.fill_angle = v,
            P::StrokeR => self.stroke[0] = c,
            P::StrokeG => self.stroke[1] = c,
            P::StrokeB => self.stroke[2] = c,
            P::StrokeA => self.stroke[3] = c,
            P::StrokeWidth => self.stroke_width = v,
            P::DashOffset => self.dash_offset = v,
            P::TrimStart => self.trim[0] = v,
            P::TrimEnd => self.trim[1] = v,
            P::TrimOffset => self.trim[2] = v,
            P::Radius => self.radius = [v; 4],
            P::InnerRadius => self.inner_radius = v,
            P::ShadowX => self.shadow.x = v,
            P::ShadowY => self.shadow.y = v,
            P::ShadowBlur => self.shadow.blur = v,
            P::ShadowSpread => self.shadow.spread = v,
            P::ShadowR => self.shadow.color[0] = c,
            P::ShadowG => self.shadow.color[1] = c,
            P::ShadowB => self.shadow.color[2] = c,
            P::ShadowA => self.shadow.color[3] = c,
            P::Progress => self.progress = v,
            P::Size => self.size = v,
            P::LetterSpacing => self.letter_spacing = v,
            P::LineHeight => self.line_height = v,
            P::TintR => self.tint[0] = c,
            P::TintG => self.tint[1] = c,
            P::TintB => self.tint[2] = c,
            P::TintA => self.tint[3] = c,
        }
    }
}

impl UnitState {
    /// The value `p` has, or None for a property units don't have.
    pub fn get(&self, p: Prop) -> Option<f64> {
        Some(match p {
            Prop::X => self.x,
            Prop::XPx => self.x_px,
            Prop::Y => self.y,
            Prop::YPx => self.y_px,
            Prop::Scale => self.scale,
            Prop::ScaleX => self.scale_x,
            Prop::ScaleY => self.scale_y,
            Prop::Rotate => self.rotate,
            Prop::Opacity => self.opacity,
            Prop::Blur => self.blur,
            _ => return None,
        })
    }

    pub fn set(&mut self, p: Prop, v: f64) {
        let v = p.clamp(v);
        match p {
            Prop::X => self.x = v,
            Prop::XPx => self.x_px = v,
            Prop::Y => self.y = v,
            Prop::YPx => self.y_px = v,
            Prop::Scale => self.scale = v,
            Prop::ScaleX => self.scale_x = v,
            Prop::ScaleY => self.scale_y = v,
            Prop::Rotate => self.rotate = v,
            Prop::Opacity => self.opacity = v,
            Prop::Blur => self.blur = v,
            _ => {}
        }
    }
}
