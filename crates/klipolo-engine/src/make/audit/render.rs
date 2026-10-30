//! The audit's small draw: a variant of the effect (`variant_source`) into `Rgba32Float` at `AUDIT_SIZE`, with the
//! effect stage's bindings (`KlFrame` and the params at 256 in one uniform buffer, the sampler, two inputs: the test
//! pattern, or nothing for a generator), read back as floats. A variant's objects are made once per draft and kept in
//! its `AuditCache`: a draw then writes its uniforms, encodes one pass and reads the target back.

mod cache;
mod objects;

pub use cache::AuditCache;

use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use super::{AUDIT_SIZE, AuditDraw, Variant, variant_source};
use crate::plan::{EffectPass, MAX_PARAM_BYTES};
use crate::registry::ShaderKind;
use crate::render::device::RenderDevice;
use crate::render::effects::{FrameInfo, frame_bytes};

const PARAMS_OFFSET: u64 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// What a draw whose readback never arrived answers.
const READ_FAILED: &str = "the graphics card didn't finish the audit draw";

/// One variant's objects, made once (wgpu's handles are shared references, so a clone is cheap).
#[derive(Clone)]
struct Prepared {
    pipeline: wgpu::RenderPipeline,
    frame_group: wgpu::BindGroup,
    input_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    target: wgpu::Texture,
    read: wgpu::Buffer,
}

/// The variant drawn, as RGBA floats, row by row.
pub(super) async fn draw(
    dev: &RenderDevice,
    d: &AuditDraw<'_>,
    variant: Variant,
) -> Result<Vec<f32>, String> {
    // The variant is taken out for this draw, so a draw at the same time prepares its own.
    let taken = d.cache.and_then(|c| {
        let mut list = c.prepared.lock().unwrap_or_else(PoisonError::into_inner);
        let at = list.iter().position(|(v, _)| *v == variant)?;
        Some(list.swap_remove(at).1)
    });
    let p = match taken {
        Some(p) => p,
        None => {
            let p = prepare(dev, d, variant)?;
            if let Some(c) = d.cache {
                c.made.fetch_add(1, Ordering::Relaxed);
            }
            p
        }
    };
    let test = d.cache.map(AuditCache::take_test).unwrap_or_default();
    let drawn = run(dev, d, &p, test).await;
    if let Some(c) = d.cache {
        let mut list = c.prepared.lock().unwrap_or_else(PoisonError::into_inner);
        match &drawn {
            // Back for the next draw, unless one drawn meanwhile already put its own back.
            Ok(_) if !list.iter().any(|(v, _)| *v == variant) => list.push((variant, p)),
            Ok(_) => {}
            // A readback may still be pending on its buffer: the draft's objects all go.
            Err(_) => list.clear(),
        }
    }
    drawn
}

/// The variant's module, pipeline, bind groups, inputs, target and buffers.
fn prepare(dev: &RenderDevice, d: &AuditDraw<'_>, variant: Variant) -> Result<Prepared, String> {
    let source = variant_source(&d.effect.source, variant)?;
    let device = &dev.device;
    let (w, h) = AUDIT_SIZE;
    // The device's error scopes are one stack: the pair holds its lock.
    let (prepared, error) = dev
        .scopes
        .scoped(device, &[wgpu::ErrorFilter::Validation], || {
            let (layout, frame_layout, input_layout) = objects::layouts(device);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Klipolo audit"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Klipolo audit"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("kl_vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("kl_fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(FORMAT.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
            let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Klipolo audit uniforms"),
                size: PARAMS_OFFSET + MAX_PARAM_BYTES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Klipolo audit frame"),
                layout: &frame_layout,
                entries: &[
                    uniform(&uniforms, 0, 64, 0),
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    uniform(&uniforms, PARAMS_OFFSET, MAX_PARAM_BYTES as u64, 2),
                ],
            });
            let reads = d.effect.kind != ShaderKind::Generator;
            let (a, b) = (
                objects::input(dev, reads, false),
                objects::input(dev, reads, true),
            );
            let (va, vb) = (
                a.create_view(&Default::default()),
                b.create_view(&Default::default()),
            );
            let input_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Klipolo audit input"),
                layout: &input_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&va),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&vb),
                    },
                ],
            });
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Klipolo audit target"),
                size: extent(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Klipolo audit readback"),
                size: u64::from(w) * 16 * u64::from(h),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            Prepared {
                pipeline,
                frame_group,
                input_group,
                uniforms,
                target,
                read,
            }
        });
    if let Some(error) = error {
        return Err(format!("the audit draw was refused: {error}"));
    }
    Ok(prepared)
}

/// One draw with `p`: this draw's uniforms, one pass, the target read back.
async fn run(
    dev: &RenderDevice,
    d: &AuditDraw<'_>,
    p: &Prepared,
    test: cache::TestRead,
) -> Result<Vec<f32>, String> {
    let (w, h) = AUDIT_SIZE;
    let mut pass = EffectPass::host(Arc::new(d.effect.clone()), d.params);
    pass.time_s = d.at;
    pass.progress = (d.at / d.duration.max(1e-3)).clamp(0.0, 1.0);
    pass.frame = Some((d.at * 30.0) as u32);
    pass.duration_s = d.duration;
    pass.fps = 30.0;
    pass.step_fps = d.effect.fps.map_or(0.0, |f| f as f32);
    let info = FrameInfo::new(0, d.size.0.min(d.size.1) as f32 / 1080.0);
    let mut bytes = vec![0u8; PARAMS_OFFSET as usize + MAX_PARAM_BYTES];
    bytes[..64].copy_from_slice(&frame_bytes(&pass, [w as f32, h as f32], info));
    let params = d.params.bytes();
    bytes[PARAMS_OFFSET as usize..PARAMS_OFFSET as usize + params.len()].copy_from_slice(params);
    dev.queue.write_buffer(&p.uniforms, 0, &bytes);
    let mut encoder = dev.device.create_command_encoder(&Default::default());
    {
        let view = p.target.create_view(&Default::default());
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Klipolo audit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_pipeline(&p.pipeline);
        rp.set_bind_group(0, &p.frame_group, &[]);
        rp.set_bind_group(1, &p.input_group, &[]);
        rp.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        p.target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &p.read,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 16),
                rows_per_image: Some(h),
            },
        },
        extent(),
    );
    dev.queue.submit([encoder.finish()]);
    let (tx, rx) = tokio::sync::oneshot::channel();
    p.read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    dev.poll_soon();
    if let Some(hold) = test.hold {
        let _ = hold.await;
    }
    if test.fail {
        return Err(READ_FAILED.into());
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
        Ok(Ok(Ok(()))) => {}
        _ => return Err(READ_FAILED.into()),
    }
    let floats = bytemuck::cast_slice::<u8, f32>(&p.read.slice(..).get_mapped_range()).to_vec();
    p.read.unmap();
    Ok(floats)
}

fn extent() -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: AUDIT_SIZE.0,
        height: AUDIT_SIZE.1,
        depth_or_array_layers: 1,
    }
}

fn uniform(
    buffer: &wgpu::Buffer,
    offset: u64,
    size: u64,
    binding: u32,
) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer,
            offset,
            size: wgpu::BufferSize::new(size),
        }),
    }
}
