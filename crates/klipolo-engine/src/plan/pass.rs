//! A shader pass as the plan hands it to the effect stage: the shader, its settings packed in WGSL's uniform layout,
//! and what its `kl` reads that the plan decides.

use std::fmt;
use std::sync::Arc;

use crate::registry::ShaderEffect;

/// The largest packed `Params` (16 settings × 16 bytes, `registry::MAX_SHADER_PARAMS`).
pub const MAX_PARAM_BYTES: usize = 256;

/// Settings packed for a shader's `Params` struct (WGSL uniform layout). Inline, so planning allocates nothing for it.
#[derive(Clone, Copy, PartialEq)]
pub struct ParamBlock {
    len: u16,
    bytes: [u8; MAX_PARAM_BYTES],
}

impl ParamBlock {
    /// `len` zero bytes (`ParamLayout::size`, at most `MAX_PARAM_BYTES`).
    pub fn zeroed(len: usize) -> Self {
        Self {
            len: len.min(MAX_PARAM_BYTES) as u16,
            bytes: [0; MAX_PARAM_BYTES],
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// Writes little-endian `values` from byte `offset`; anything past the end is dropped.
    pub fn put_f32s(&mut self, offset: usize, values: &[f32]) {
        for (k, v) in values.iter().enumerate() {
            self.put(offset + 4 * k, &v.to_le_bytes());
        }
    }

    pub fn put(&mut self, offset: usize, data: &[u8]) {
        if let Some(dst) = self.bytes.get_mut(offset..offset + data.len()) {
            dst.copy_from_slice(data);
        }
    }
}

impl fmt::Debug for ParamBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ParamBlock({:02x?})", self.bytes())
    }
}

/// One shader pass over a layer.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectPass {
    pub shader: Arc<ShaderEffect>,
    pub params: ParamBlock,
    /// Seconds since the layer started: timeline time, never the wall clock, so preview and export match. A generator
    /// layer's passes follow its piece of the contribution's time, held on its `fps` steps (`generator_time`).
    pub time_s: f32,
    /// 0–1 across the layer.
    pub progress: f32,
    /// What its `kl.frame` reads: a generator layer's piece frame on the 30 fps grain clock (`GRAIN_FPS`,
    /// `generator_time`), whatever the project's rate; None, the timeline frame the compositor counts (the same clock).
    pub frame: Option<u32>,
    /// `kl.duration` (ABI v1.1): the seconds `progress` runs across, the layer's length (a split generator's whole
    /// span), or a transition's window; 0 for the host's own passes.
    pub duration_s: f32,
    /// `kl.fps` (ABI v1.1): the project's frame rate; 0 for the host's own passes.
    pub fps: f32,
    /// `kl.step_fps` (ABI v1.1): the rate its time steps at, its generator's `fps`; 0 when its time runs smoothly.
    pub step_fps: f32,
    /// Stable per layer and effect (`seed_of`).
    pub seed: u32,
    /// How far it draws past its layer's box, in 1080p pixels: its contribution's `padding` for these settings.
    pub pad_px: f32,
}

impl EffectPass {
    /// A pass the host runs for itself (resample, grab, PAD, the overlay blend): `shader` with `params`, at no time,
    /// seed or margin.
    pub fn host(shader: Arc<ShaderEffect>, params: ParamBlock) -> Self {
        Self {
            shader,
            params,
            time_s: 0.0,
            progress: 0.0,
            frame: None,
            duration_s: 0.0,
            fps: 0.0,
            step_fps: 0.0,
            seed: 0,
            pad_px: 0.0,
        }
    }
}
