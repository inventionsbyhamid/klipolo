//! What a shader's output check hides (worker only): the effect drawn with a raw entry, its output not
//! sanitised, into `Rgba32Float` at 128 × 72, and the pixels counted that the check would have replaced: non-finite,
//! out of 0–1, or `rgb > a`. The same small draw also counts a pixel's loop trips (the host's shared counter,
//! `kl_trips`), which the fake meter charges per trip so tests measure deterministically.
//!
//! A variant is the hardened text with only the output check's body replaced, so every loop keeps its counter
//! check: it is parsed, validated and its loop guards counted again before any device sees it, and fails closed. It is
//! drawn only here, on the worker's own device; nothing else ever compiles it. A draft keeps its variants' pipelines
//! (`AuditCache`), so only the uniforms change from one draw to the next.

mod render;

use std::collections::BTreeMap;

pub use render::AuditCache;

use naga::valid::{Capabilities, ValidationFlags, Validator};

use crate::plan::{ParamBlock, pack_params};
use crate::registry::ShaderEffect;
use crate::registry::guard::harden::loop_guards;
use crate::render::device::RenderDevice;

/// The audit's target.
pub const AUDIT_SIZE: (u32, u32) = (128, 72);

/// What a variant's entry returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// The effect's own output, unsanitised.
    Raw,
    /// The pixel's loop trips in red.
    Trips,
}

/// The pixels the output check would have replaced, by why (a pixel counts once, for the first that applies).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unsanitised {
    pub non_finite: u64,
    pub out_of_range: u64,
    pub over_alpha: u64,
}

impl Unsanitised {
    pub fn total(&self) -> u64 {
        self.non_finite + self.out_of_range + self.over_alpha
    }
}

/// One draw of an effect at a time: what it reads and when.
pub struct AuditDraw<'a> {
    pub effect: &'a ShaderEffect,
    pub params: ParamBlock,
    /// The frame the effect is drawn for (its `kl.px` follows it).
    pub size: (u32, u32),
    /// Seconds into the layer.
    pub at: f32,
    /// The layer's length (`kl.progress` runs across it).
    pub duration: f32,
    /// The draft's own cache of compiled variants; None compiles them for this draw alone.
    pub cache: Option<&'a AuditCache>,
}

/// The pixels of `effect` (at its defaults) at `at` s in a frame of `size` that the output check replaces, counted at
/// 128 × 72; 0 when it can't be drawn (the frames say why).
pub async fn count_unsanitised(
    device: &RenderDevice,
    effect: &ShaderEffect,
    size: (u32, u32),
    at: f32,
) -> u64 {
    let draw = AuditDraw {
        effect,
        params: pack_params(effect, &BTreeMap::new()),
        size,
        at,
        duration: crate::make::draft::DRAFT_SECONDS as f32,
        cache: None,
    };
    audit(device, &draw).await.map_or(0, |u| u.total())
}

/// [`count_unsanitised`] by why, at any settings.
pub async fn audit(device: &RenderDevice, draw: &AuditDraw<'_>) -> Result<Unsanitised, String> {
    let texels = render::draw(device, draw, Variant::Raw).await?;
    let mut u = Unsanitised::default();
    for p in texels.chunks_exact(4) {
        if p.iter().any(|c| !c.is_finite()) {
            u.non_finite += 1;
        } else if p.iter().any(|c| !(0.0..=1.0).contains(c)) {
            u.out_of_range += 1;
        } else if p[..3].iter().any(|c| *c > p[3] + 1e-4) {
            u.over_alpha += 1;
        }
    }
    Ok(u)
}

/// The mean loop trips a pixel of `draw` runs (the host's counter, at most `LOOP_TRIPS`), over 128 × 72.
pub async fn trips_per_pixel(device: &RenderDevice, draw: &AuditDraw<'_>) -> Result<f64, String> {
    let texels = render::draw(device, draw, Variant::Trips).await?;
    let n = (texels.len() / 4).max(1) as f64;
    Ok(texels
        .chunks_exact(4)
        .map(|p| f64::from(p[0].max(0.0)))
        .sum::<f64>()
        / n)
}

/// `hardened` (a checked shader's text) with its output check's body replaced for `variant`; refused unless it still
/// parses, validates and keeps every loop's counter check.
pub fn variant_source(hardened: &str, variant: Variant) -> Result<String, String> {
    const HEAD: &str = "fn kl_sanitize(";
    let start = hardened.find(HEAD).ok_or("no output check")?;
    let open = start + hardened[start..].find('{').ok_or("no output check body")?;
    let mut depth = 0usize;
    let mut end = None;
    for (i, c) in hardened[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.ok_or("an unclosed output check")?;
    let param = hardened[start + HEAD.len()..]
        .split(':')
        .next()
        .map(str::trim)
        .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .ok_or("an output check without its parameter")?;
    let body = match variant {
        Variant::Raw => format!("return {param};"),
        Variant::Trips => "return vec4<f32>(f32(kl_trips), 0.0, 0.0, 1.0);".into(),
    };
    let text = format!(
        "{}fn kl_sanitize({param}: vec4<f32>) -> vec4<f32> {{\n    {body}\n}}{}",
        &hardened[..start],
        &hardened[end..]
    );
    let module = naga::front::wgsl::parse_str(&text).map_err(|e| e.emit_to_string(&text))?;
    Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .map_err(|e| e.emit_to_string(&text))?;
    let g = loop_guards(&module);
    if g.guarded != g.loops {
        return Err(format!(
            "{} of its {} loops lost their counter check",
            g.loops - g.guarded,
            g.loops
        ));
    }
    Ok(text)
}
