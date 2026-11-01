//! Shader contributions on the WGSL effect ABI (v1.1). A contribution supplies one function; the host assembles the
//! prelude, a `Params` struct generated from the schema, the body and an entry point once, and packs values into
//! `Params` per frame (`plan::pack_params`). The authors' contract is the comment at the top of
//! `wgsl/prelude.wgsl`. Who wrote a shader decides its text (`trust`); shaders that aren't built in are assembled,
//! checked and hardened by `untrusted`. A generator's size and frame rate are checked by `timing`.

use std::fmt;
use std::fmt::Write as _;
use std::sync::Arc;

use klipolo_model::contrib::{ContribDef, ContribId, PaddingSpec, Resolution};
use klipolo_model::params::{ParamKind, ParamSpec};

use crate::make::budget::RunsAt;
use crate::make::envelope::Envelope;
use crate::registry::guard::StaticFacts;

mod downsample;
mod timing;
mod trust;
mod untrusted;
pub use downsample::Downsample;
pub use timing::MAX_STEP_FPS;
pub use trust::ShaderTrust;
pub use untrusted::SANITIZE;
pub(crate) use untrusted::source as untrusted_source;

/// The effect ABI's major version. Additive changes keep it; anything that would change how an existing shader
/// looks bumps it, and the host keeps an adapter for the previous major.
pub const EFFECT_ABI: u32 = 1;
/// The effect ABI's minor version: 1 since `kl.fps`, `kl.duration`, `kl.resolution` and `kl.step_fps` joined `KlFrame`,
/// with generator time that follows a split layer's piece, stepped time, cached static generators and half-size ones.
/// Shaders written for v1.0 draw the same. A contribution's key includes it (`registry::key`), so a minor change checks
/// every shader that isn't built in again.
pub const EFFECT_ABI_MINOR: u32 = 1;
pub const PRELUDE: &str = include_str!("wgsl/prelude.wgsl");
/// 16 settings of at most 16 bytes fill `plan::MAX_PARAM_BYTES`.
pub const MAX_SHADER_PARAMS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderKind {
    /// `fn effect(uv) -> vec4<f32>` over the layer (`kl_a`).
    Filter,
    /// `fn effect(uv) -> vec4<f32>` with no input.
    Generator,
    /// `fn transition(uv) -> vec4<f32>` from `kl_a` to `kl_b` by `kl.progress`.
    Transition,
}

/// The WGSL type a setting packs into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    F32,
    I32,
    U32,
    Vec2,
    Vec4,
}

impl Slot {
    /// Size in bytes, which is also the alignment for these types in WGSL's uniform layout.
    pub fn size(self) -> usize {
        match self {
            Self::F32 | Self::I32 | Self::U32 => 4,
            Self::Vec2 => 8,
            Self::Vec4 => 16,
        }
    }

    fn wgsl(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::I32 => "i32",
            Self::U32 => "u32",
            Self::Vec2 => "vec2<f32>",
            Self::Vec4 => "vec4<f32>",
        }
    }
}

/// Where each setting sits in `Params`: schema order, each member aligned to its size.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamLayout {
    /// (index into the schema, slot, byte offset).
    pub fields: Vec<(usize, Slot, usize)>,
    /// The struct's size: a multiple of 16, at least 16.
    pub size: usize,
}

impl ParamLayout {
    pub fn of(params: &[ParamSpec]) -> Result<Self, String> {
        if params.len() > MAX_SHADER_PARAMS {
            return Err(format!(
                "A shader effect can have up to {MAX_SHADER_PARAMS} settings."
            ));
        }
        let mut fields = Vec::with_capacity(params.len());
        let mut offset: usize = 0;
        for (i, spec) in params.iter().enumerate() {
            if !shader_key(&spec.key) {
                return Err(format!(
                    "\"{}\" can't name a shader setting: use up to 32 letters and digits, starting with a lowercase letter.",
                    spec.key
                ));
            }
            let slot = match &spec.kind {
                ParamKind::Float { .. } | ParamKind::Angle { .. } => Slot::F32,
                ParamKind::Int { .. } => Slot::I32,
                ParamKind::Bool { .. } | ParamKind::Choice { .. } => Slot::U32,
                ParamKind::Point { .. } => Slot::Vec2,
                ParamKind::Color { .. } => Slot::Vec4,
                ParamKind::Text { .. }
                | ParamKind::Asset { .. }
                | ParamKind::Font { .. }
                | ParamKind::Table { .. } => {
                    return Err(format!(
                        "Shader effects can't take text, media, font or table settings ({}).",
                        spec.key
                    ));
                }
            };
            offset = offset.next_multiple_of(slot.size());
            fields.push((i, slot, offset));
            offset += slot.size();
        }
        Ok(Self {
            fields,
            size: offset.next_multiple_of(16).max(16),
        })
    }

    fn wgsl(&self, params: &[ParamSpec]) -> String {
        let mut out = String::from("struct Params {\n");
        if self.fields.is_empty() {
            out.push_str("    _unused: vec4<f32>,\n");
        }
        for &(i, slot, _) in &self.fields {
            let _ = writeln!(out, "    {}: {},", params[i].key, slot.wgsl());
        }
        out.push_str("};\n@group(0) @binding(2) var<uniform> params: Params;\n");
        out
    }
}

/// Letters and digits, starting lowercase, 32 at most, never `kl…` (the host's names). A WGSL reserved word is
/// caught by naga at load with its line.
fn shader_key(key: &str) -> bool {
    key.len() <= 32
        && key.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && key.bytes().all(|b| b.is_ascii_alphanumeric())
        && !key.starts_with("kl")
}

/// One shader contribution, assembled and ready to compile. A clone shares its text (`Arc`); the render worker draws
/// clones with other timing for its measurements (`make::draft`).
#[derive(Clone)]
pub struct ShaderEffect {
    pub id: ContribId,
    pub name: String,
    pub kind: ShaderKind,
    /// Prelude + `Params` + body + entry point.
    pub source: Arc<str>,
    /// `fnv1a64(source)`: the pipeline cache key (an edited file is a new pipeline; hot reload relies on it).
    pub key: u64,
    pub params: Arc<[ParamSpec]>,
    pub layout: ParamLayout,
    /// A second pass the host draws right after this one with the same settings: separable built-ins such as blur
    /// (across, then down). Plugin shaders have none in ABI v1.
    pub then: Option<Arc<ShaderEffect>>,
    /// When the pass may run at half size (the definition's `downsample`, resolved against `layout`).
    pub downsample: Option<Downsample>,
    /// It hides what it covers (the definition's `protective` marker: blur, pixelate, the shape mask), so the frame's
    /// budget never drops it (`effects::passes::protects`).
    pub protective: bool,
    /// How far past its layer's box a filter draws (the definition's `padding`): its layer's effect target and quad
    /// grow by it (`plan::EffectPass::pad_px`). None for anything but a filter.
    pub padding: Option<PaddingSpec>,
    /// The size a generator draws at (its definition's `resolution`): `Half` draws it at half size, `(w + 1) / 2`,
    /// and the quad scales it up; `Auto` is drawn as `Full` until the render worker resolves it. `Full` for
    /// anything but a generator.
    pub resolution: Resolution,
    /// A generator's frame rate (its definition's `fps`, 1–60): its time holds on `1/fps` steps
    /// (`plan::generator_time`), so it draws again only when the step changes. None for anything but a generator.
    pub fps: Option<u32>,
    /// Who wrote it: a built-in, or a shader that passed the guard and whose `source` is hardened.
    pub trust: ShaderTrust,
    /// It reads `kl.time`, `kl.progress` or `kl.frame` (or the whole `kl`), itself or through a function it calls:
    /// read from its IR (the guard's facts). One that doesn't draws the same every frame.
    pub animates: bool,
    /// A checked shader's static facts; None for a built-in.
    pub facts: Option<Arc<StaticFacts>>,
    /// The settings this computer measured it within, from its verdict (`verified`): the plan
    /// keeps a layer's costly settings inside it (`plan::effects::clamped`). None for a built-in or an unchecked shader.
    pub envelope: Option<Arc<Envelope>>,
    /// How its verdict runs it: `verified` has already put half size into `resolution` or `downsample`; kept for
    /// what reads it (the export job, the crash guard). Full size, every frame, for a built-in.
    pub runs_at: RunsAt,
    /// The cost this computer verified it at (`verified`): its class's budget for the preview's sampler,
    /// its export times for the export's tiles. None for a built-in or an unchecked shader.
    pub cost: Option<Arc<crate::make::budget::Cost>>,
}

impl ShaderEffect {
    pub fn assemble(def: &ContribDef, kind: ShaderKind, body: &str) -> Result<Self, String> {
        def.check_conditions()?;
        let layout = ParamLayout::of(&def.params)?;
        let source = layout.source(def, kind, body, ShaderTrust::Builtin);
        let (resolution, fps) = timing::timing(def, kind).map_err(|r| r.message)?;
        Ok(Self {
            key: fnv1a64(source.as_bytes()),
            source: source.into(),
            id: def.id.clone(),
            name: def.name.clone(),
            kind,
            params: def.params.clone().into(),
            downsample: Downsample::resolve(def, kind, &layout)?,
            protective: def.protective,
            padding: def.padding.clone().filter(|_| kind == ShaderKind::Filter),
            resolution,
            fps,
            layout,
            then: None,
            trust: ShaderTrust::Builtin,
            // Until `builtin` reads it: a shader not known to stand still is drawn every frame.
            animates: true,
            facts: None,
            envelope: None,
            runs_at: RunsAt::default(),
            cost: None,
        })
    }

    /// This shader followed by `then` (same contribution and settings).
    pub fn followed_by(mut self, then: ShaderEffect) -> Self {
        self.then = Some(Arc::new(then));
        self
    }
}

impl PartialEq for ShaderEffect {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.id == other.id
    }
}

impl fmt::Debug for ShaderEffect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShaderEffect({}, {:016x})", self.id, self.key)
    }
}

/// FNV-1a, 64 bit: the same on every run and build (pipeline keys, effect seeds).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_on(0xcbf2_9ce4_8422_2325, bytes)
}

/// [`fnv1a64`] continued from `hash` over `bytes`, so a key over several parts needs no buffer (generator cache keys).
pub fn fnv1a64_on(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Parses and validates WGSL with naga, so a broken shader reports its line instead of a GPU error.
pub fn validate_wgsl(source: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(source))?;
    Ok(())
}
