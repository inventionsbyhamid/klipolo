//! Export tiles. An export draws at full size whatever the preview did, and a
//! GPU that runs one draw for seconds stalls the whole machine (WindowServer on macOS, the watchdog's reset). So an
//! export's `Checked` pass predicted over [`TILE_MS`] is drawn in horizontal bands, each scissored, each its own
//! submission that waits for the one before: as many as keep each under [`TILE_MS`] by the prediction, at most
//! [`MAX_TILES`]. The prediction is the larger of the verified export time at this size (`Cost::export_ms`, scaled by
//! pixels from 1080p and 2160p) and the static one (`StaticFacts::static_ms`), so a branch the measurement never
//! took can't become one long 4K draw either. The bands make exactly the same image: the first clears the target, the
//! rest load it, and each draws only its rows.

use crate::make::budget::Cost;
use crate::registry::{ShaderEffect, ShaderTrust};
use crate::render::device::RenderDevice;

use super::padding::Inner;

/// No band is predicted over this.
pub const TILE_MS: f64 = 200.0;
/// The most bands one pass is cut into.
pub const MAX_TILES: u32 = 64;

const P1080: f64 = 1920.0 * 1080.0;
const P2160: f64 = 3840.0 * 2160.0;

/// `cost`'s export time for a pass of `pixels`: the larger of its 1080p and 2160p figures, each scaled by pixels.
pub fn export_ms_at(cost: &Cost, pixels: f64) -> f64 {
    (cost.export_ms.p1080 * pixels / P1080).max(cost.export_ms.p2160 * pixels / P2160)
}

/// How many bands an export draws `shader`'s pass of `size` in: 1 unless it is `Checked` and predicted over
/// [`TILE_MS`] (a prediction that isn't a number is taken as the most).
pub fn tiles_for(shader: &ShaderEffect, size: (u32, u32)) -> u32 {
    if shader.trust != ShaderTrust::Checked {
        return 1;
    }
    let pixels = f64::from(size.0) * f64::from(size.1);
    let measured = shader
        .cost
        .as_deref()
        .map_or(0.0, |c| export_ms_at(c, pixels));
    let predicted = shader
        .facts
        .as_ref()
        .map_or(0.0, |f| f.static_ms(pixels as u64));
    let worst = measured.max(predicted);
    if worst.is_nan() || worst.is_infinite() {
        return MAX_TILES;
    }
    if worst <= TILE_MS {
        return 1;
    }
    ((worst / TILE_MS).ceil() as u32).clamp(1, MAX_TILES)
}

/// What one tiled pass draws with.
pub(super) struct TiledPass<'a> {
    pub pipeline: &'a wgpu::RenderPipeline,
    pub view: &'a wgpu::TextureView,
    pub frame: &'a wgpu::BindGroup,
    pub inputs: &'a wgpu::BindGroup,
    pub inner: Option<Inner>,
    /// The target's size.
    pub size: (u32, u32),
}

/// Draws `pass` in `tiles` bands of rows, each its own submission, each waiting for the one before; what `encoder`
/// holds so far (what the pass reads) is submitted first, and `encoder` is a new one afterwards. The submissions made.
pub(super) fn tiled_pass(
    dev: &RenderDevice,
    encoder: &mut wgpu::CommandEncoder,
    pass: TiledPass<'_>,
    tiles: u32,
) -> u32 {
    let (device, queue) = (&dev.device, &dev.queue);
    let fresh = || {
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Klipolo compositor"),
        })
    };
    let before = std::mem::replace(encoder, fresh());
    queue.submit([before.finish()]);
    let (w, h) = pass.size;
    let mut submitted = 0;
    for i in 0..tiles {
        let (y0, y1) = (h * i / tiles, h * (i + 1) / tiles);
        if y1 <= y0 {
            continue;
        }
        let mut band = fresh();
        {
            let mut rp = band.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Klipolo effect tile"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: pass.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if submitted == 0 {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(pass.pipeline);
            if let Some(r) = pass.inner {
                rp.set_viewport(r.x, r.y, r.w, r.h, 0.0, 1.0);
            }
            rp.set_scissor_rect(0, y0, w, y1 - y0);
            rp.set_bind_group(0, pass.frame, &[]);
            rp.set_bind_group(1, pass.inputs, &[]);
            rp.draw(0..3, 0..1);
        }
        let index = queue.submit([band.finish()]);
        // The next band waits for this one: the GPU is never handed more than a band's work at once.
        if let Err(error) = device.poll(wgpu::PollType::WaitForSubmissionIndex(index)) {
            tracing::warn!(%error, "an effect tile didn't finish");
        }
        submitted += 1;
    }
    submitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ShaderKind;
    use klipolo_model::contrib::{ContribDef, ContribKind};

    fn checked(body: &str, export_1080: f64) -> ShaderEffect {
        let def = ContribDef::new("lib.k3f9q2ma/generator.t.v1", ContribKind::Generator, "T");
        let shader = ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, body).unwrap();
        let verdict =
            crate::registry::pack::Verdict::of(Cost::with_export_ms(export_1080)).unwrap();
        shader.verified(&verdict)
    }

    const FLAT: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";

    #[test]
    fn tiles_keep_each_band_under_200_ms_by_the_larger_prediction() {
        let hd = (1920, 1080);
        assert_eq!(tiles_for(&checked(FLAT, 50.0), hd), 1);
        assert_eq!(tiles_for(&checked(FLAT, 200.0), hd), 1);
        assert_eq!(tiles_for(&checked(FLAT, 900.0), hd), 5);
        // At 4K, the 2160p figure (4 × 1080p unless measured) at its own pixels.
        assert_eq!(tiles_for(&checked(FLAT, 900.0), (3840, 2160)), 18);
        // At most 64.
        assert_eq!(tiles_for(&checked(FLAT, 1e9), hd), MAX_TILES);
        // Measured cheap, statically heavy: the static prediction decides.
        let gated = "fn effect(uv: vec2<f32>) -> vec4<f32> {
            var c = vec4<f32>(uv, 0.0, 1.0);
            if (uv.x > 2.0) {
                for (var i = 0; i < 4000; i++) {
                    c = vec4<f32>(c.rgb * 0.999 + vec3<f32>(sin(f32(i) * uv.x) * 0.001), 1.0);
                }
            }
            return clamp(c, vec4<f32>(0.0), vec4<f32>(1.0));
        }";
        assert!(tiles_for(&checked(gated, 1.0), hd) > 1);
        // A built-in is never tiled.
        let builtin = crate::registry::Registry::builtin();
        assert!(builtin.shaders().all(|s| tiles_for(s, (7680, 4320)) == 1));
    }
}
