//! The plan's output: what each layer draws, back to front, in canvas-normalized units. Ids, paths and texts are
//! shared strings (`plan::strings`): planning a frame copies none.

use std::sync::Arc;

use klipolo_model::contrib::ContribKind;
use klipolo_model::layout::BoxN;
use klipolo_model::motion::Blend;
use klipolo_model::project::FormatKind;
use klipolo_model::time::Us;
use klipolo_model::timeline::TextStyle;

use super::EffectPass;

mod cursor;
mod video;

pub use cursor::{CursorDraw, CursorLayer, CursorMask, Ripple};
pub use video::{LookPlan, LutRef, SourceRef, VideoLayer, VideoRole};

#[derive(Clone, Debug, PartialEq)]
pub struct FramePlan {
    pub format: FormatKind,
    /// The format's canvas in pixels; renders at other sizes scale from it.
    pub canvas: (u32, u32),
    pub at_us: Us,
    pub background: BackgroundPlan,
    /// Back to front.
    pub layers: Vec<Layer>,
    /// Contributions this frame uses that aren't installed, with their kind. They draw as nothing (a caption style
    /// draws in Clean), the compositor reports each once, and the project keeps them.
    pub missing: Vec<MissingContrib>,
}

/// A contribution a frame uses that this build doesn't have, and its kind: a caption style is said by name as
/// `frame.captionStyleMissing` (its captions draw in Clean); anything else as a missing plugin. One the catalogue knows
/// that is being checked (its check is pending, its settings were clamped while they are measured again,
/// or the "Agent-made effects" switch leaves it out) is `checking`, said as `frame.agentEffectChecking`, and never stops
/// an export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingContrib {
    pub kind: ContribKind,
    pub id: Arc<str>,
    pub checking: bool,
}

impl FramePlan {
    /// Every video layer, in the order their frames are decoded and drawn: layer order, with a transition's outgoing
    /// side before its incoming side. Allocates nothing (the decoder pool walks it per frame).
    pub fn videos(&self) -> impl Iterator<Item = &VideoLayer> + Clone {
        video_layers(&self.layers)
    }

    /// Video sources this frame needs, in `videos()` order.
    pub fn sources(&self) -> Vec<&SourceRef> {
        self.videos().map(|v| &v.source).collect()
    }

    /// Video sources with the role that shows them (decoder budgets differ by role), in `videos()` order.
    pub fn video_sources(&self) -> Vec<(VideoRole, &SourceRef)> {
        self.videos().map(|v| (v.role, &v.source)).collect()
    }

    /// This plan with its titles alone: what the playback lookahead warms a title's start with, so
    /// warming a title opens no decoder (the plan at a title's start a second ahead holds the clip already playing,
    /// past the pool's forward window, which would move its spare lane there). The perf harness warms the same.
    pub fn titles_only(mut self) -> FramePlan {
        self.layers.retain(|l| matches!(l, Layer::Text(_)));
        self
    }
}

/// The video layers of `layers`, a transition's sides included. A side holds one clip's layers (`plan::clip_layers`),
/// never another transition, so one level of nesting is all there is.
fn video_layers(layers: &[Layer]) -> impl Iterator<Item = &VideoLayer> + Clone {
    layers.iter().flat_map(|layer| {
        let (own, sides): (Option<&VideoLayer>, [&[Layer]; 2]) = match layer {
            Layer::Video(v) => (Some(v), [&[], &[]]),
            Layer::Transition(t) => (None, [&t.outgoing, &t.incoming]),
            _ => (None, [&[], &[]]),
        };
        own.into_iter()
            .chain(sides.into_iter().flatten().filter_map(|l| match l {
                Layer::Video(v) => Some(v),
                _ => None,
            }))
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundPlan {
    Solid {
        color: [u8; 3],
    },
    Gradient {
        from: [u8; 3],
        to: [u8; 3],
        angle_deg: f64,
    },
    /// Drawn to cover the canvas.
    Image {
        asset_id: Arc<str>,
        path: Arc<str>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Layer {
    Video(VideoLayer),
    Image(ImageLayer),
    Text(TextLayer),
    Generator(GeneratorLayer),
    /// A motion graphic (`plan::scene`).
    Scene(super::scene::SceneLayer),
    Adjustment(AdjustmentLayer),
    /// Full-canvas black at `alpha` over everything drawn so far (clip fades).
    Black {
        alpha: f64,
    },
    /// Two main clips blended across their cut.
    Transition(TransitionLayer),
    /// The captions, over everything else (`plan::captions`; boxed: a page's words are a fixed-size array).
    Caption(Box<super::captions::CaptionLayer>),
    /// A screen take's cursor, over its clip's main footage (the cursor pass draws it). Never a video layer.
    Cursor(CursorLayer),
}

/// Two clips blended across their cut: each side's main-track layers (footage, camera, fades) drawn alone, then
/// joined by `pass`, a transition shader (`kl_a` outgoing, `kl_b` incoming, `kl.progress` across the window).
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionLayer {
    /// The outgoing clip's id.
    pub id: Arc<str>,
    pub outgoing: Vec<Layer>,
    pub incoming: Vec<Layer>,
    pub pass: EffectPass,
    /// The window opens on the outgoing clip's first frame (its first half covers the whole clip): nothing plays the
    /// outgoing clip before it, so the playback lookahead warms both sides, not only the incoming one.
    pub outgoing_starts: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageLayer {
    pub id: Arc<str>,
    pub asset_id: Arc<str>,
    pub path: Arc<str>,
    /// The image contained in its layout box (keeps its shape).
    pub dest: BoxN,
    pub rotation: f64,
    pub opacity: f64,
    pub radius: f64,
    pub effects: Vec<EffectPass>,
    /// As `VideoLayer::pad_px`.
    pub pad_px: f64,
    pub blend: Blend,
    /// As `VideoLayer::reach`: its decode and effect targets are sized for the largest box its motion reaches.
    pub reach: f64,
}

/// Whose text a text layer is. The text drawer keeps each text's texture under its owner and id. Captions aren't text
/// layers (`Layer::Caption`, drawn by the caption renderer), so every text layer is an overlay track's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextOwner {
    /// A text layer of an overlay track (`id` is the item's).
    Layer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextLayer {
    pub id: Arc<str>,
    pub owner: TextOwner,
    /// What to draw now (the typewriter preset shows a prefix while it runs).
    pub text: Arc<str>,
    pub style: Arc<TextStyle>,
    /// Font size in canvas pixels (`style.size` is pixels at a 1080 shorter side).
    pub font_px: f64,
    pub dest: BoxN,
    pub rotation: f64,
    pub opacity: f64,
    /// Uniform scale about the centre of `dest` (motion: pop, zoom, pulse, scale keys). `dest` keeps the box's own
    /// size, so the text is laid out once and the quad scales it.
    pub scale: f64,
    /// How much of the text is shown, 0–1 (the typewriter preset; `text` is already cut to it); 1 otherwise.
    pub reveal: f64,
    pub effects: Vec<EffectPass>,
    /// As `VideoLayer::pad_px`: its texture's room around the text grows by it (no extra pass).
    pub pad_px: f64,
    pub blend: Blend,
    /// The largest `scale` its motion reaches (at least 1; the same on every frame): the text is laid out once at
    /// about that size, so a title keyed up stays sharp (`render::text`).
    pub reach: f64,
    /// The text language its Chinese, Japanese and Korean follow (the context's `text_language`).
    pub language: Arc<str>,
}

/// A picture a generator draws in `dest`, placed like an image.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratorLayer {
    pub id: Arc<str>,
    pub dest: BoxN,
    pub rotation: f64,
    pub opacity: f64,
    pub radius: f64,
    /// Its own pass, timed to its piece (`plan::generator_time`).
    pub source: EffectPass,
    /// Some when `source` draws the same picture as on a previous frame with the same key (it reads no time, or its
    /// time holds on `fps` steps): the compositor then reuses that picture (`render::effects::statics`).
    pub cache_key: Option<u64>,
    /// It draws at half size (`resolution: half`), `(w + 1) / 2` each way, with its filters, and the quad scales
    /// it up.
    pub half: bool,
    pub effects: Vec<EffectPass>,
    /// As `VideoLayer::pad_px` (its `effects`'): the generator draws straight into the grown target (no extra pass).
    pub pad_px: f64,
    pub blend: Blend,
    /// As `VideoLayer::reach`.
    pub reach: f64,
}

/// What is already drawn inside `dest` (snapped to whole pixels), re-drawn through `effects`.
#[derive(Clone, Debug, PartialEq)]
pub struct AdjustmentLayer {
    pub id: Arc<str>,
    pub dest: BoxN,
    pub opacity: f64,
    /// Never empty (an adjustment without effects isn't planned).
    pub effects: Vec<EffectPass>,
}
