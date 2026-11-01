//! What each animated property name becomes: one or more numeric tracks. A position is a fraction of the box and 1080p
//! pixels added together (as CSS `calc()` mixes them), so `x` is two tracks; a colour is four (straight RGBA, 0–1);
//! `origin` is four; `clip` and `text` are tracks of indices into the node's clips and texts. Each track's values are
//! held within its field's range after modifiers (the check's `ranges`).

use arrayvec::ArrayVec;

use super::super::check::range_of;
use super::super::eval::tracks::CHAIN_DEPTH;
use super::super::units::{MAX_FRACTION, MAX_PX};

/// One animated number of a node (or of each of a split text's units).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Prop {
    X,
    XPx,
    Y,
    YPx,
    OriginX,
    OriginXPx,
    OriginY,
    OriginYPx,
    Scale,
    ScaleX,
    ScaleY,
    Rotate,
    RotateX,
    RotateY,
    Perspective,
    Opacity,
    /// 0 or 1.
    Visible,
    Blur,
    /// An index into the node's clips.
    Clip,
    FillR,
    FillG,
    FillB,
    FillA,
    FillOffset,
    FillAngle,
    StrokeR,
    StrokeG,
    StrokeB,
    StrokeA,
    StrokeWidth,
    DashOffset,
    TrimStart,
    TrimEnd,
    TrimOffset,
    Radius,
    InnerRadius,
    ShadowX,
    ShadowY,
    ShadowBlur,
    ShadowSpread,
    ShadowR,
    ShadowG,
    ShadowB,
    ShadowA,
    /// `motionPath.progress`.
    Progress,
    Size,
    LetterSpacing,
    LineHeight,
    /// An index into the node's texts.
    Text,
    TintR,
    TintG,
    TintB,
    TintA,
}

/// How a property name's value is read into tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lower {
    /// A fraction of the box and pixels.
    Pos(Prop, Prop),
    Num(Prop),
    Color([Prop; 4]),
    Flag(Prop),
    Origin,
    Clip,
    Text,
}

impl Lower {
    /// The tracks it writes, one per number.
    pub(crate) fn props(self) -> ArrayVec<Prop, 4> {
        let mut out = ArrayVec::new();
        match self {
            Lower::Pos(f, p) => out.extend([f, p]),
            Lower::Num(p) | Lower::Flag(p) => out.push(p),
            Lower::Color(ps) => out.extend(ps),
            Lower::Origin => out.extend([
                Prop::OriginX,
                Prop::OriginXPx,
                Prop::OriginY,
                Prop::OriginYPx,
            ]),
            Lower::Clip => out.push(Prop::Clip),
            Lower::Text => out.push(Prop::Text),
        }
        out
    }
}

/// The most tracks a frame reads for one property's run of `g` tracks, `c` of them chained: the run, then for each
/// chained read a shorter run before it, at most `CHAIN_DEPTH` deep. The checker (`scene.tooManyTracks`) and each
/// instance count with it.
pub(crate) fn run_reads(g: usize, c: usize) -> usize {
    let deep = c.min(usize::from(CHAIN_DEPTH)).min(g.saturating_sub(1));
    (0..=deep).map(|d| g - d).sum::<usize>() * usize::from(g > 0)
}

const FILL: [Prop; 4] = [Prop::FillR, Prop::FillG, Prop::FillB, Prop::FillA];
const STROKE: [Prop; 4] = [Prop::StrokeR, Prop::StrokeG, Prop::StrokeB, Prop::StrokeA];
const SHADOW: [Prop; 4] = [Prop::ShadowR, Prop::ShadowG, Prop::ShadowB, Prop::ShadowA];
const TINT: [Prop; 4] = [Prop::TintR, Prop::TintG, Prop::TintB, Prop::TintA];

/// The tracks an animated property name writes (the `keys::ANIMATABLE` names).
pub(crate) fn lower_of(name: &str) -> Option<Lower> {
    use Prop as P;
    Some(match name {
        "x" => Lower::Pos(P::X, P::XPx),
        "y" => Lower::Pos(P::Y, P::YPx),
        "origin" => Lower::Origin,
        "visible" => Lower::Flag(P::Visible),
        "clip" => Lower::Clip,
        "text" => Lower::Text,
        "fill" => Lower::Color(FILL),
        "stroke" => Lower::Color(STROKE),
        "shadow.color" => Lower::Color(SHADOW),
        "tint" => Lower::Color(TINT),
        _ => Lower::Num(match name {
            "scale" => P::Scale,
            "scaleX" => P::ScaleX,
            "scaleY" => P::ScaleY,
            "rotate" => P::Rotate,
            "rotateX" => P::RotateX,
            "rotateY" => P::RotateY,
            "perspective" => P::Perspective,
            "opacity" => P::Opacity,
            "blur" => P::Blur,
            "fill.offset" => P::FillOffset,
            "fill.angle" => P::FillAngle,
            "strokeWidth" => P::StrokeWidth,
            "dashOffset" => P::DashOffset,
            "trim.start" => P::TrimStart,
            "trim.end" => P::TrimEnd,
            "trim.offset" => P::TrimOffset,
            "radius" => P::Radius,
            "innerRadius" => P::InnerRadius,
            "shadow.x" => P::ShadowX,
            "shadow.y" => P::ShadowY,
            "shadow.blur" => P::ShadowBlur,
            "shadow.spread" => P::ShadowSpread,
            "motionPath.progress" => P::Progress,
            "size" => P::Size,
            "letterSpacing" => P::LetterSpacing,
            "lineHeight" => P::LineHeight,
            _ => return None,
        }),
    })
}

impl Prop {
    /// Whether its values switch at keys instead of easing: on/off and texts.
    pub fn switches(self) -> bool {
        matches!(self, Self::Visible | Self::Text)
    }

    /// Whether each unit of a split text has its own: a staggered tween or the text's own staggered keys move units.
    pub fn per_unit(self) -> bool {
        matches!(
            self,
            Self::X
                | Self::XPx
                | Self::Y
                | Self::YPx
                | Self::Scale
                | Self::ScaleX
                | Self::ScaleY
                | Self::Rotate
                | Self::Opacity
                | Self::Blur
        )
    }

    /// The values it is held within once eased and modified.
    pub fn range(self) -> (f64, f64) {
        use Prop as P;
        let name = match self {
            P::X | P::Y => "x",
            P::XPx | P::YPx | P::OriginXPx | P::OriginYPx => return (-MAX_PX, MAX_PX),
            P::OriginX | P::OriginY => return (-MAX_FRACTION, MAX_FRACTION),
            P::Visible
            | P::FillR
            | P::FillG
            | P::FillB
            | P::FillA
            | P::StrokeR
            | P::StrokeG
            | P::StrokeB
            | P::StrokeA
            | P::ShadowR
            | P::ShadowG
            | P::ShadowB
            | P::ShadowA
            | P::TintR
            | P::TintG
            | P::TintB
            | P::TintA => return (0.0, 1.0),
            P::Clip | P::Text => return (0.0, f64::from(u16::MAX)),
            P::Scale => "scale",
            P::ScaleX => "scaleX",
            P::ScaleY => "scaleY",
            P::Rotate => "rotate",
            P::RotateX => "rotateX",
            P::RotateY => "rotateY",
            P::Perspective => "perspective",
            P::Opacity => "opacity",
            P::Blur => "blur",
            P::FillOffset => "fill.offset",
            P::FillAngle => "fill.angle",
            P::StrokeWidth => "strokeWidth",
            P::DashOffset => "dashOffset",
            P::TrimStart => "trim.start",
            P::TrimEnd => "trim.end",
            P::TrimOffset => "trim.offset",
            P::Radius => "radius",
            P::InnerRadius => "innerRadius",
            P::ShadowX => "shadow.x",
            P::ShadowY => "shadow.y",
            P::ShadowBlur => "shadow.blur",
            P::ShadowSpread => "shadow.spread",
            P::Progress => "motionPath.progress",
            P::Size => "size",
            P::LetterSpacing => "letterSpacing",
            P::LineHeight => "lineHeight",
        };
        range_of(name).unwrap_or((-f64::MAX, f64::MAX))
    }

    /// `v` within [`Prop::range`]; anything not finite is the range's nearest end to 0.
    pub fn clamp(self, v: f64) -> f64 {
        let (lo, hi) = self.range();
        if v.is_nan() {
            0.0f64.clamp(lo, hi)
        } else {
            v.clamp(lo, hi)
        }
    }
}
