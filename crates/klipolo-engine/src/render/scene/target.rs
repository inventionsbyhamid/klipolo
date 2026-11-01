//! A scene's own pass: its frame drawn into a pooled `EFFECT_FORMAT` target of the effect stage at the
//! layer's drawn size (its motion's reach included), cleared transparent: each run of shapes instanced in one draw,
//! each image on its own with its picture bound, each path's fill or stroke from its mesh (a translucent
//! stroke through the pass's stencil, so it blends once a pixel), each run of text from the instance's
//! atlas. The layer's
//! effects then run on the target and the compositor places it like any layer. A frame the scene holds still for keeps
//! its target (hold caching): see [`covers`].

use super::cache::Held;
use super::meshes::Meshes;
use super::pack::Run;
use super::path_pipeline::{PathBuffers, PathPipeline};
use super::shapes::{ShapeBuffers, ShapePipeline};
use super::text_pipeline::{TextGpu, TextPipeline};

/// What a scene's pass draws with: both pipelines, the instance's shapes and path draws, its path meshes and its
/// pictures' bind groups (`pictures[k]` for the layer's picture `k`; None draws it not at all).
pub(super) struct Pass<'a> {
    pub shapes: &'a ShapePipeline,
    pub paths: &'a PathPipeline,
    pub gpu: &'a ShapeBuffers,
    pub draws: &'a PathBuffers,
    pub meshes: &'a Meshes,
    pub pictures: &'a [Option<wgpu::BindGroup>],
    /// The text pipeline and the instance's text at this size, when it has text.
    pub text: Option<(&'a TextPipeline, &'a TextGpu)>,
    /// The frame's generator fills' and backdrops' targets bound (None draws that shape not at all).
    pub fills: &'a [Option<wgpu::BindGroup>],
    pub backdrops: &'a [Option<wgpu::BindGroup>],
}

/// Which pipeline a pass has set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum On {
    Shapes,
    Paths,
    Text,
}

/// One target a pass draws into: its view, its stencil (the same size) when its runs draw a translucent stroke
/// ([`needs_stencil`]), and its slot (`slots.rs`: where it sits in the layer's target; 0 the scene's own).
#[derive(Clone, Copy)]
pub(super) struct Onto<'a> {
    pub view: &'a wgpu::TextureView,
    pub stencil: Option<&'a wgpu::TextureView>,
    pub slot: usize,
}

/// Whether `runs` draw a translucent stroke, which blends once a pixel through a stencil: only then is one
/// attached to their pass, the pipelines that state it with it.
pub(super) fn needs_stencil(runs: &[Run]) -> bool {
    runs.iter()
        .any(|r| matches!(r, Run::Path { once: true, .. }))
}

/// Encodes `runs` into `into` (cleared first when `clear`): the first `shapes` shapes and `draws` path draws in `runs`'
/// order, switching pipelines between runs of shapes and path draws. A translucent stroke takes the next stencil
/// reference; past 255 of them the pass ends and another goes on drawing over it with its stencil cleared. An
/// offscreen node's and a mask's markers are the caller's (`encode.rs`) and draw nothing here.
pub(super) fn encode(
    encoder: &mut wgpu::CommandEncoder,
    into: Onto,
    with: &Pass,
    counts: (u32, u32, u32),
    runs: &[Run],
    clear: bool,
) {
    let mut from = 0;
    let mut first = true;
    while first || from < runs.len() {
        from += encode_pass(encoder, into, with, counts, &runs[from..], first && clear);
        first = false;
    }
}

/// One pass over `runs` (clearing the target when `first`): how many runs it drew.
fn encode_pass(
    encoder: &mut wgpu::CommandEncoder,
    into: Onto,
    with: &Pass,
    (shapes, draws, texts): (u32, u32, u32),
    runs: &[Run],
    first: bool,
) -> usize {
    let view = into.view;
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Klipolo scene"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if first {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: into
            .stencil
            .map(|view| wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: None,
                stencil_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0),
                    store: wgpu::StoreOp::Discard,
                }),
            }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    // The pipelines that state the pass's stencil, or their twins without one.
    let stencil = into.stencil.is_some();
    let pick = |with, without| if stencil { with } else { without };
    let mut on: Option<On> = None;
    let mut reference = 0u32;
    for (i, run) in runs.iter().enumerate() {
        let want = match run {
            Run::Path { .. } => On::Paths,
            Run::Text(..) => On::Text,
            Run::Begin(_) | Run::End(_) | Run::Mask(_) | Run::MaskEnd(_) => continue,
            _ => On::Shapes,
        };
        if on != Some(want) {
            pass.set_bind_group(0, &with.gpu.group, &[super::slots::offset(into.slot)]);
            match (want, with.text) {
                (On::Paths, _) => {
                    pass.set_pipeline(pick(&with.paths.pipeline, &with.paths.plain));
                    pass.set_bind_group(1, &with.draws.group, &[]);
                }
                (On::Text, Some((pipeline, gpu))) => {
                    pass.set_pipeline(pick(&pipeline.pipeline, &pipeline.plain));
                    pass.set_bind_group(1, gpu.group(), &[]);
                }
                (On::Text, None) => continue,
                (On::Shapes, _) => {
                    pass.set_pipeline(pick(&with.shapes.pipeline, &with.shapes.plain));
                    pass.set_vertex_buffer(0, with.gpu.instances.slice(..));
                }
            }
            on = Some(want);
        }
        match *run {
            Run::Text(start, end) => {
                let end = end.min(texts);
                if start < end {
                    pass.draw(0..6, start..end);
                }
            }
            Run::Shapes(start, end) => {
                let end = end.min(shapes);
                if start < end {
                    pass.set_bind_group(1, &with.shapes.pictures.none, &[]);
                    pass.draw(0..6, start..end);
                }
            }
            Run::Image { from, to, picture } => {
                let group = with.pictures.get(usize::from(picture));
                bound(&mut pass, group, (from, to.min(shapes)));
            }
            Run::Fill { from, to, fill } => {
                bound(
                    &mut pass,
                    with.fills.get(usize::from(fill)),
                    (from, to.min(shapes)),
                );
            }
            Run::Backdrop { from, to, slot } => {
                let group = with.backdrops.get(usize::from(slot));
                bound(&mut pass, group, (from, to.min(shapes)));
            }
            Run::Begin(_) | Run::End(_) | Run::Mask(_) | Run::MaskEnd(_) => {}
            Run::Path {
                node,
                draw,
                stroke,
                once,
            } => {
                let mesh = with.meshes.get(node as usize).and_then(|m| m.mesh.as_ref());
                let Some(mesh) = mesh.filter(|_| draw < draws) else {
                    continue;
                };
                let (start, end) = if stroke { mesh.stroke } else { mesh.fill };
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint16);
                if !once || !stencil {
                    pass.draw_indexed(start..end, 0, draw..draw + 1);
                    continue;
                }
                if reference == 255 {
                    // Every reference is taken: the next pass clears the stencil and draws on from here.
                    return i;
                }
                reference += 1;
                pass.set_stencil_reference(reference);
                pass.set_pipeline(&with.paths.solid);
                pass.draw_indexed(start..end, 0, draw..draw + 1);
                pass.set_pipeline(&with.paths.edge);
                pass.draw_indexed(start..end, 0, draw..draw + 1);
                pass.set_pipeline(&with.paths.pipeline);
            }
        }
    }
    runs.len()
}

/// Shapes `[from, to)` drawn with `group` bound as their picture (an image's, a generator fill's or a backdrop's);
/// nothing without one.
fn bound(
    pass: &mut wgpu::RenderPass,
    group: Option<&Option<wgpu::BindGroup>>,
    (from, to): (u32, u32),
) {
    if let Some(group) = group.and_then(Option::as_ref).filter(|_| from < to) {
        pass.set_bind_group(1, group, &[]);
        pass.draw(0..6, from..to);
    }
}

/// Whether the frame `held` shows is the frame at scene time `t` too: the same time, or a later one before the scene
/// next changes, or an earlier one from which nothing changes until it. `until_from` gives when the scene next changes
/// after a time (`static_until`).
pub(super) fn covers(held: &Held, t: f64, until_from: impl FnOnce(f64) -> f64) -> bool {
    if t == held.t {
        return true;
    }
    if t > held.t {
        return t < held.until;
    }
    until_from(t) > held.t
}
