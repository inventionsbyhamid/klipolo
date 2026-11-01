//! What every pass of one frame shares, with the layer's own box in the passes' uv that padding added, and the
//! `KlFrame` uniform a pass reads, built from it and the pass (ABI v1.1 adds `fps`, `duration`, `resolution` and
//! `step_fps`).

use bytemuck::{Pod, Zeroable};

use crate::plan::EffectPass;

/// What every pass of one frame shares.
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    /// The timeline frame index (the grain clock).
    pub frame: u32,
    /// Output pixels per 1080p pixel.
    pub px: f32,
    /// The layer's own box in the passes' uv (`kl.content`): [`WHOLE`](FrameInfo::WHOLE) unless a padded filter grew
    /// the target, so shapes, centres and grids keep to the layer.
    pub content: [f32; 4],
    /// `kl.resolution` (ABI v1.1): 1, halved by each halving: 0.5 for passes at half size (a `resolution: half`
    /// generator and its filters, or a stack a downsampled filter halved), 0.25 when both apply. `px` includes it.
    pub resolution: f32,
}

impl FrameInfo {
    /// The whole target is the layer.
    pub const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    /// `frame` at `px`, the whole target the layer, at full resolution.
    pub fn new(frame: u32, px: f32) -> Self {
        Self {
            frame,
            px,
            content: Self::WHOLE,
            resolution: 1.0,
        }
    }

    /// The same at half the resolution: `px` and `resolution` halved.
    pub fn halved(self) -> Self {
        Self {
            px: self.px / 2.0,
            resolution: self.resolution / 2.0,
            ..self
        }
    }
}

/// The prelude's `KlFrame` (`registry/wgsl/prelude.wgsl`), as the effect stage writes it at offset 0 of a pass's
/// uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct KlFrame {
    size: [f32; 2],
    time: f32,
    progress: f32,
    frame: u32,
    seed: u32,
    px: f32,
    _pad: f32,
    /// The layer's own box in the pass's uv (`FrameInfo::content`).
    content: [f32; 4],
    /// v1.1: `EffectPass::fps` (the project's rate), `EffectPass::duration_s`, `FrameInfo::resolution` and
    /// `EffectPass::step_fps` (a generator's step rate), filling the 16 bytes from 48.
    fps: f32,
    duration: f32,
    resolution: f32,
    step_fps: f32,
}

// The prelude's WGSL `KlFrame` is 64 bytes: v1's 48 with `content` at 32, then v1.1's four f32s from 48 (additive, a
// multiple of WGSL's 16-byte uniform alignment), `step_fps` last at 60. A field added on one side only fails the build here, or
// `kl_frame_is_the_preludes_struct` below, instead of a draw.
const _: () = assert!(std::mem::size_of::<KlFrame>() == 64);
const _: () = assert!(std::mem::offset_of!(KlFrame, content) == 32);
const _: () = assert!(std::mem::offset_of!(KlFrame, fps) == 48);
const _: () = assert!(std::mem::offset_of!(KlFrame, step_fps) == 60);

/// What `pass` reads as `kl`, drawn `size` pixels big (a padded layer's inner rect may be fractional). Its `frame` is
/// the pass's own when the plan set one (a generator layer's piece frame), else the frame's.
pub(super) fn kl_frame(pass: &EffectPass, size: [f32; 2], info: FrameInfo) -> KlFrame {
    KlFrame {
        size,
        time: pass.time_s,
        progress: pass.progress,
        frame: pass.frame.unwrap_or(info.frame),
        seed: pass.seed,
        px: info.px,
        _pad: 0.0,
        content: info.content,
        fps: pass.fps,
        duration: pass.duration_s,
        resolution: info.resolution,
        step_fps: pass.step_fps,
    }
}

/// [`kl_frame`] as the bytes a uniform buffer holds, for the render worker's own small draws (`make::audit`).
pub(crate) fn frame_bytes(pass: &EffectPass, size: [f32; 2], info: FrameInfo) -> [u8; 64] {
    bytemuck::cast(kl_frame(pass, size, info))
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::KlFrame;
    use crate::registry::Registry;

    /// The uniform the effect stage writes is the struct every shader reads: naga's layout of the prelude's `KlFrame`,
    /// its size and each member's name and offset, is the Rust one's.
    #[test]
    fn kl_frame_is_the_preludes_struct() {
        let registry = Registry::builtin();
        let shader = registry.shaders().next().expect("a built-in shader");
        let module = naga::front::wgsl::parse_str(&shader.source).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("KlFrame"))
            .expect("the prelude's KlFrame");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("KlFrame is a struct");
        };
        assert_eq!(*span as usize, size_of::<KlFrame>());
        let wgsl: Vec<(&str, usize)> = members
            .iter()
            .map(|m| (m.name.as_deref().unwrap_or(""), m.offset as usize))
            .collect();
        let rust = [
            ("size", offset_of!(KlFrame, size)),
            ("time", offset_of!(KlFrame, time)),
            ("progress", offset_of!(KlFrame, progress)),
            ("frame", offset_of!(KlFrame, frame)),
            ("seed", offset_of!(KlFrame, seed)),
            ("px", offset_of!(KlFrame, px)),
            ("_pad", offset_of!(KlFrame, _pad)),
            ("content", offset_of!(KlFrame, content)),
            ("fps", offset_of!(KlFrame, fps)),
            ("duration", offset_of!(KlFrame, duration)),
            ("resolution", offset_of!(KlFrame, resolution)),
            ("step_fps", offset_of!(KlFrame, step_fps)),
        ];
        assert_eq!(wgsl, rust);
        // Whatever varies with time is one of the members the guard's `animates`
        // reads; every other member holds over a layer's frames, so a generator reading only those is static. A new
        // member fails here until it is put in one list or the other.
        let held = [
            "size",
            "seed",
            "px",
            "_pad",
            "content",
            "fps",
            "duration",
            "resolution",
            "step_fps",
        ];
        for (name, _) in &wgsl {
            assert!(
                crate::registry::guard::TIME_MEMBERS.contains(name) != held.contains(name),
                "KlFrame.{name} must be either time or held, not both or neither"
            );
        }
    }
}
