// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/layers/text.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted (TextAlign/TextBackgroundStyle and PreparedText from vendor::cap::rendering::text, log::warn -> tracing::warn); Cap's readiness phase timing removed from `new`; the per-text shaping parameters (wrap slack, metrics, family, weight, attrs, align) moved from prepare_with_mode into a `shaping` helper, unchanged, so the new `laid_out_height` (Klipolo sizes a text layer's texture with it) shapes exactly as prepare does; `has_family` added (Klipolo falls back to sans-serif for fonts that aren't installed); the GPU tests module removed (it needs pollster and Cap's instance helper); `with_font_system`, `set_font_system` and `trim_atlas` added (the font scan never blocks the preview, and glyphon's atlas is trimmed after each text is drawn); `warm_background` added (makes the text-box pipeline ahead of the first boxed text: the GPU warm-up at Home). The shadow pass falls from the outlined letters when there is an outline (`shadow_offsets`), so a shadow shows past the outline instead of hiding under it.; `shaping` judges the wrap slack against `PreparedText::layout_width` when set (a text drawn into a window of its layout keeps the layout of the whole); the text's own pass colours `PreparedText::highlight`'s bytes through the same per-byte rich-text path as a gradient (Karaoke captions); the atlas uses glyphon's Web colour mode, not its default Accurate one (the compositor's textures hold sRGB values in Rgba8Unorm, so Accurate drew every colour but white and black darker than its hex); a text with `PreparedText::runs` is shaped with one rich-text span per run, each asking for its run's family at the text's own weight and style, the per-byte colours carried in the same spans (`styled_spans`, which replaces `rich_spans`), and `has_family` is gone (the engine resolves faces with `klipolo_fonts::resolve`); `shaping` asks the face's width (`stretch`) and clamps the weight to 1–1000, not 100–900; `shape` wraps with `Wrap::WordOrGlyph`, not `Wrap::Word`, so a word wider than the box breaks inside it instead of being clipped; the wrap slack's rule moved from `shaping` into `wrap_width`, unchanged, so a test holds the webview to it; the layer no longer owns a font system: `prepare`, `prepare_mixed` and `laid_out_height` take the compositor's shared `&mut FontSystem` (`render::shared_fonts`), and `with_font_system` and `set_font_system` are gone; `reset_atlas` added, and the layer keeps its glyphon `Cache` for it (glyphon keys rasterised glyphs by font id, and a new font system numbers its faces anew); `stroke_offsets` puts the outermost ring's copies at most 2 px apart and the inner rings' `max(2, √(2r))` px apart (`dense_ring`, 8 to 128 a ring) instead of 8 a ring, which stamped visible steps on thick outlines; `areas_last_prepare` and `atlas_resets` added (the outline test's and the perf harness's counters); outlines are drawn as one glyph a letter, the letter grown by the outline (`outline_glyphs`: glyphon custom glyphs from `render::outline_glyphs`, swash strokes), not stamped copies, and an outlined shadow is those glyphs moved: `stroke_offsets`, `dense_ring`, `shadow_offsets` and their tests are gone, `Pass::outline` and the layer's `outlines`, `custom` and `empty` are added, and the prepares use `prepare_with_custom`; `laid_out_height` became `laid_out_extent`, which also gives the ink's horizontal extent (a title's texture is cut to its ink), and `bound_atlas` makes the glyph atlas anew after a text whose glyphs needed more than `GLYPH_ATLAS_SIDE`² (estimated per prepare: `atlas_area`). An outlined shadow moves a whole pixel across, so its outline glyphs are the outline's own, and `outline_counts` is added.

use std::ops::Range;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glyphon::cosmic_text::Align;
use glyphon::{
    Attrs, Buffer, Cache, Color, ColorMode, CustomGlyph, Family, FontSystem, Metrics, Resolution,
    Shaping, Style, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};
use tracing::warn;
use wgpu::{Device, Queue, include_wgsl, util::DeviceExt};

use crate::render::outline_glyphs::OutlineGlyphs;
use crate::vendor::cap::rendering::text::{
    PreparedText, StaggerEdge, TextAlign, TextBackgroundStyle, stagger_alpha,
};

pub struct TextLayer {
    cache: Cache,
    swash_cache: SwashCache,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,
    buffers: Vec<Buffer>,
    draws: Vec<TextDraw>,
    segment_renderers: Vec<TextRenderer>,
    background: Option<TextBackgroundResources>,
    segmented_render: bool,
    /// Klipolo: the text areas the last prepare drew (a stamped copy is one), and how many times the atlas was reset.
    areas: usize,
    resets: u64,
    /// Klipolo: outlines as glyphs of their own (one quad a letter, not stamped copies), this prepare's
    /// outline glyphs (each outline pass has its range), and the empty buffer their areas draw with.
    outlines: OutlineGlyphs,
    custom: Vec<CustomGlyph>,
    empty: Buffer,
    /// Klipolo: the atlas area the last prepare's glyphs need (`atlas_area`), and its scratch.
    atlas_area: u64,
    seen: Seen,
}

/// Klipolo: the glyph atlas is made anew after a text needing more than this side's
/// square holds (a mask atlas of 4 MiB; glyphon grows it to 8192² otherwise: 64 MiB, and 256 MiB for colour).
pub const GLYPH_ATLAS_SIDE: u32 = 2048;
/// The glyphs' area that may need more than `GLYPH_ATLAS_SIDE`² once packed (packing fills half of it, at worst).
const GLYPH_ATLAS_AREA: u64 = (GLYPH_ATLAS_SIDE as u64 * GLYPH_ATLAS_SIDE as u64) / 2;

/// Klipolo: the atlas area a prepare's glyphs need, about: each distinct glyph of `buffers` (face, glyph and size)
/// its advance by 1.3 em, scaled, a pixel round; each distinct outline glyph its own size.
fn atlas_area(buffers: &[Buffer], custom: &[CustomGlyph], scale: f32, seen: &mut Seen) -> u64 {
    seen.0.clear();
    seen.1.clear();
    let mut area = 0u64;
    for run in buffers.iter().flat_map(|b| b.layout_runs()) {
        for glyph in run.glyphs {
            if seen
                .0
                .insert((glyph.font_id, glyph.glyph_id, glyph.font_size.to_bits()))
            {
                let w = (glyph.w * scale).ceil().max(0.0) as u64 + 2;
                let h = (glyph.font_size * scale * 1.3).ceil().max(0.0) as u64 + 2;
                area += w * h;
            }
        }
    }
    for glyph in custom {
        if seen.1.insert(glyph.id) {
            area += (glyph.width * scale).ceil().max(0.0) as u64
                * (glyph.height * scale).ceil().max(0.0) as u64;
        }
    }
    area
}

/// `atlas_area`'s scratch: the glyphs and outline glyphs counted.
type Seen = (
    std::collections::HashSet<(glyphon::cosmic_text::fontdb::ID, u16, u32)>,
    std::collections::HashSet<u16>,
);

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct TextBackgroundUniforms {
    rect: [f32; 4],
    color: [f32; 4],
    radius: f32,
    _padding0: f32,
    _padding1: f32,
    _padding2: f32,
    output_size: [f32; 2],
    _padding3: [f32; 2],
}

struct TextBackgroundResources {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffers: Vec<wgpu::Buffer>,
    bind_groups: Vec<wgpu::BindGroup>,
}

impl TextBackgroundResources {
    fn new(device: &Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Text Background Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shader = device.create_shader_module(include_wgsl!("../shaders/text_bg.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Text Background Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Text Background Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
            uniform_buffers: Vec::new(),
            bind_groups: Vec::new(),
        }
    }

    fn ensure_capacity(&mut self, device: &Device, count: usize) {
        while self.uniform_buffers.len() < count {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Text Background Uniform Buffer"),
                contents: bytemuck::bytes_of(&TextBackgroundUniforms {
                    rect: [0.0; 4],
                    color: [0.0; 4],
                    radius: 0.0,
                    _padding0: 0.0,
                    _padding1: 0.0,
                    _padding2: 0.0,
                    output_size: [1.0; 2],
                    _padding3: [0.0; 2],
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Text Background Bind Group"),
                layout: &self.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.uniform_buffers.push(buffer);
            self.bind_groups.push(bind_group);
        }
    }

    fn write(&self, queue: &Queue, index: usize, uniforms: &TextBackgroundUniforms) {
        queue.write_buffer(
            &self.uniform_buffers[index],
            0,
            bytemuck::bytes_of(uniforms),
        );
    }

    fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, index: usize) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_groups[index], &[]);
        pass.draw(0..6, 0..1);
    }
}

/// One `TextArea` per entry of `offsets`, all sharing `buffer` and tinted
/// `color` unless the buffer carries its own per-glyph colours. Klipolo: an
/// outline pass (`outline`: its glyphs in the layer's `custom` and how far
/// they reach past the text) draws `buffer`'s letters grown by the outline,
/// one glyph each, at its one offset.
struct Pass {
    buffer: usize,
    color: Color,
    offsets: Vec<[f32; 2]>,
    outline: Option<(Range<usize>, i32)>,
}

struct TextDraw {
    track: u32,
    bounds: TextBounds,
    left: f32,
    top: f32,
    scale: f32,
    /// Draw order: shadow, glow, outline, then the text itself.
    passes: Vec<Pass>,
    background: Range<usize>,
}

fn text_areas<'a>(
    buffers: &'a [Buffer],
    empty: &'a Buffer,
    custom: &'a [CustomGlyph],
    draw: &'a TextDraw,
) -> impl Iterator<Item = TextArea<'a>> + 'a {
    draw.passes.iter().flat_map(move |pass| {
        pass.offsets.iter().map(move |[dx, dy]| {
            let bounds = shift_bounds(draw.bounds, *dx, *dy);
            match &pass.outline {
                // Klipolo: the outline glyphs alone, clipped to the bounds grown by the outline (the stamped copies
                // were each clipped to their own shifted bounds).
                Some((glyphs, grow)) => TextArea {
                    buffer: empty,
                    left: draw.left + dx,
                    top: draw.top + dy,
                    scale: draw.scale,
                    bounds: TextBounds {
                        left: bounds.left - grow,
                        top: bounds.top - grow,
                        right: bounds.right + grow,
                        bottom: bounds.bottom + grow,
                    },
                    default_color: pass.color,
                    custom_glyphs: &custom[glyphs.clone()],
                },
                None => TextArea {
                    buffer: &buffers[pass.buffer],
                    left: draw.left + dx,
                    top: draw.top + dy,
                    scale: draw.scale,
                    bounds,
                    default_color: pass.color,
                    custom_glyphs: &[],
                },
            }
        })
    })
}

/// Klipolo: `buffer`'s letters grown by `width` px (scaled), as outline glyphs for an area at `left`,
/// `top` and `scale`, appended to `custom`; their range and how far they reach past the letters, whole pixels. Each
/// is placed at its letter's own pixel and subpixel bin, so the outline sits exactly round the fill.
#[allow(clippy::too_many_arguments)]
fn outline_glyphs(
    outlines: &mut OutlineGlyphs,
    custom: &mut Vec<CustomGlyph>,
    fs: &mut FontSystem,
    buffer: &Buffer,
    left: f32,
    top: f32,
    scale: f32,
    width: f32,
) -> (Range<usize>, i32) {
    let start = custom.len();
    for run in buffer.layout_runs() {
        let line_y = (run.line_y * scale).round() as i32;
        for glyph in run.glyphs {
            let physical = glyph.physical((left, top), scale);
            let Some((id, at)) = outlines.glyph(fs, physical.cache_key, width * scale) else {
                continue;
            };
            let (x, y) = (physical.x + at.left, line_y + physical.y - at.top);
            custom.push(CustomGlyph {
                id,
                left: (x as f32 - left) / scale,
                top: (y as f32 - top) / scale,
                width: at.width as f32 / scale,
                height: at.height as f32 / scale,
                color: glyph.color_opt,
                snap_to_physical_pixel: true,
                metadata: glyph.metadata,
            });
        }
    }
    (start..custom.len(), (width * scale).ceil() as i32 + 1)
}

fn shift_bounds(bounds: TextBounds, dx: f32, dy: f32) -> TextBounds {
    TextBounds {
        left: bounds.left + dx.floor() as i32,
        top: bounds.top + dy.floor() as i32,
        right: bounds.right + dx.ceil() as i32,
        bottom: bounds.bottom + dy.ceil() as i32,
    }
}

fn to_color(rgb: [f32; 4], alpha: f32) -> Color {
    Color::rgba(
        (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Eight compass directions at `radius`, the diagonals pulled in so every
/// copy sits on the same circle.
fn ring(radius: f32) -> impl Iterator<Item = [f32; 2]> {
    let diagonal = radius * std::f32::consts::FRAC_1_SQRT_2;
    [
        [radius, 0.0],
        [-radius, 0.0],
        [0.0, radius],
        [0.0, -radius],
        [diagonal, diagonal],
        [-diagonal, diagonal],
        [diagonal, -diagonal],
        [-diagonal, -diagonal],
    ]
    .into_iter()
}

/// Radius (em) and alpha of each halo ring, outermost faintest.
const GLOW_RINGS: [(f32, f32); 3] = [(0.04, 0.16), (0.09, 0.10), (0.16, 0.06)];

/// Which reveal unit each byte of `content` belongs to, and how many units
/// there are. Whitespace rides with the unit before it.
fn unit_map(content: &str, by_word: bool) -> (Vec<usize>, usize) {
    let mut map = vec![0; content.len()];
    let mut units = 0usize;
    let mut in_word = false;
    for (index, ch) in content.char_indices() {
        if ch.is_whitespace() {
            in_word = false;
        } else if !by_word || !in_word {
            units += 1;
            in_word = true;
        }
        let unit = units.saturating_sub(1);
        for byte in map.iter_mut().take(index + ch.len_utf8()).skip(index) {
            *byte = unit;
        }
    }
    (map, units)
}

fn stagger_alpha_by_byte(content: &str, edge: Option<StaggerEdge>, out: &mut [f32]) {
    let Some(edge) = edge else {
        return;
    };
    if edge.progress >= 1.0 {
        return;
    }
    let (map, units) = unit_map(content, edge.by_word);
    for (alpha, unit) in out.iter_mut().zip(map) {
        *alpha *= stagger_alpha(edge, unit, units);
    }
}

/// Byte offset of every original line's start, matching cosmic-text's own
/// line split so a glyph's in-line byte range maps back into `content`.
fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn lerp_rgb(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3],
    ]
}

/// Per-byte text colour for a horizontal gradient: each laid-out line runs
/// `from` at its left ink edge to `to` at its right.
fn gradient_by_byte(buffer: &Buffer, content: &str, from: [f32; 4], to: [f32; 4]) -> Vec<[f32; 4]> {
    let mut colors = vec![from; content.len()];
    let starts = line_starts(content);
    for run in buffer.layout_runs() {
        let Some(base) = starts.get(run.line_i).copied() else {
            continue;
        };
        let (min_x, max_x) = run
            .glyphs
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), glyph| {
                (lo.min(glyph.x), hi.max(glyph.x + glyph.w))
            });
        let span = (max_x - min_x).max(1.0);
        for glyph in run.glyphs {
            let t = (glyph.x + glyph.w * 0.5 - min_x) / span;
            let color = lerp_rgb(from, to, t);
            for byte in colors
                .iter_mut()
                .take((base + glyph.end).min(content.len()))
                .skip(base + glyph.start)
            {
                *byte = color;
            }
        }
    }
    colors
}

/// A text's runs (each a byte range and the family it asks for), as `PreparedText::runs` holds them.
type Runs = [(Range<usize>, Arc<str>)];
/// One rich-text span: its bytes, its colour (None: the default) and its run (an index in `Runs`; None outside them).
type Span = (Range<usize>, Option<Color>, Option<usize>);

/// Coalesces per-byte colours (None: the default colour) and the text's runs into the fewest rich-text spans that
/// rebuild `content` exactly, which is what `set_rich_text` needs: each span has one colour and one run (its index in
/// `runs`; None outside every run).
fn styled_spans(content: &str, colors: Option<&[Color]>, runs: &Runs) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut run = 0;
    for (index, _) in content.char_indices() {
        let color = colors.map(|colors| colors[index]);
        while run < runs.len() && runs[run].0.end <= index {
            run += 1;
        }
        let family = (run < runs.len() && runs[run].0.contains(&index)).then_some(run);
        match spans.last_mut() {
            Some((range, c, f)) if *c == color && *f == family => range.end = index,
            _ => spans.push((index..index, color, family)),
        }
    }
    let mut end = content.len();
    for (range, _, _) in spans.iter_mut().rev() {
        range.end = end;
        end = range.start;
    }
    spans
}

/// The ink extent of one laid-out line: `(left, right)` from the leftmost
/// glyph edge to the rightmost, in unscaled buffer space, or `None` for an
/// empty line.
fn run_ink(run: &glyphon::LayoutRun<'_>) -> Option<(f32, f32)> {
    run.glyphs
        .iter()
        .fold(None, |extent: Option<(f32, f32)>, glyph| {
            let (lo, hi) = extent.unwrap_or((f32::MAX, f32::MIN));
            Some((lo.min(glyph.x), hi.max(glyph.x + glyph.w)))
        })
}

/// Shape with a little more width than the editor-measured box:
/// the webview and cosmic-text can disagree by a few pixels per
/// line, and without slack a line that fit in the editor wraps in
/// the render. The room is placed so the aligned edge stays put —
/// split for centered text, after for left, before for right.
/// Boxes already spanning the frame keep their exact width — there
/// the editor genuinely wrapped too. (Klipolo: `output_width` is the text's own texture, its box plus padding, which a
/// box never fills, so a Klipolo text always wraps at box × 1.05 + 4: `textBox.wrapWidth` in the webview.)
pub(crate) fn wrap_width(width: f32, output_width: f32) -> f32 {
    if width < output_width * 0.98 {
        (width * 1.05 + 4.0).min(output_width.max(width))
    } else {
        width
    }
}

/// The shaping parameters `prepare` uses for one text: attributes, alignment, metrics, the wrap width (with the
/// editor slack) and how far the block's origin moves left for that slack.
fn shaping(text: &PreparedText, output_width: f32) -> (Attrs<'_>, Align, Metrics, f32, f32) {
    // Klipolo: a texture that is a window of a larger layout wraps as the whole would.
    let output_width = text.layout_width.unwrap_or(output_width);
    let width = (text.bounds[2] - text.bounds[0]).max(1.0);
    let wrap_width = wrap_width(width, output_width);
    let origin_dx = match text.align {
        TextAlign::Left => 0.0,
        TextAlign::Center => (wrap_width - width) / 2.0,
        TextAlign::Right => wrap_width - width,
    };

    let metrics = Metrics::new(text.font_size, text.font_size * text.line_height);
    let family = match text.font_family.trim() {
        "" => Family::SansSerif,
        name => match name.to_ascii_lowercase().as_str() {
            "sans" | "sans-serif" | "system sans" | "system sans-serif" => Family::SansSerif,
            "serif" | "system serif" => Family::Serif,
            "mono" | "monospace" | "system mono" | "system monospace" => Family::Monospace,
            _ => Family::Name(name),
        },
    };
    // Klipolo: 1–1000, the weights faces have (an installed 950 is asked as itself).
    let weight = Weight(text.font_weight.round().clamp(1.0, 1000.0) as u16);
    // Glyph colour comes from each area's default_color (or, for a
    // gradient / staggered reveal, from per-span attrs) so the
    // shadow, glow and outline passes can re-tint the same layout.
    let mut attrs = Attrs::new()
        .family(family)
        .weight(weight)
        .stretch(crate::render::fonts::width_of(text.stretch))
        .style(if text.italic {
            Style::Italic
        } else {
            Style::Normal
        });
    if text.letter_spacing != 0.0 {
        // cosmic-text adds letter_spacing to the em-relative glyph
        // advance and multiplies by font size at layout (shape.rs), so
        // the attr is in em — convert from our px value.
        attrs = attrs.letter_spacing(text.letter_spacing / text.font_size.max(1.0));
    }
    let align = match text.align {
        TextAlign::Left => Align::Left,
        TextAlign::Center => Align::Center,
        TextAlign::Right => Align::Right,
    };
    (attrs, align, metrics, wrap_width, origin_dx)
}

impl TextLayer {
    /// Klipolo: the layer shapes with the font system each call is given (the compositor's one).
    pub fn new(device: &Device, queue: &Queue) -> Self {
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        // Klipolo: the web colour model, as every other texture of the compositor (sRGB values in Rgba8Unorm). glyphon's
        // default (Accurate) turns each glyph's colour linear for an sRGB target, so in these textures every colour but
        // white and black drew darker than its hex (#FFD400 as #FFA800).
        let mut text_atlas = TextAtlas::with_color_mode(
            device,
            queue,
            &cache,
            wgpu::TextureFormat::Rgba8Unorm,
            ColorMode::Web,
        );
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            device,
            wgpu::MultisampleState::default(),
            None,
        );

        Self {
            cache,
            swash_cache,
            text_atlas,
            text_renderer,
            viewport,
            buffers: Vec::new(),
            draws: Vec::new(),
            segment_renderers: Vec::new(),
            background: None,
            segmented_render: false,
            areas: 0,
            resets: 0,
            outlines: OutlineGlyphs::default(),
            custom: Vec::new(),
            empty: Buffer::new_empty(Metrics::new(1.0, 1.0)),
            atlas_area: 0,
            seen: Default::default(),
        }
    }

    /// Klipolo: how tall `text` lays out (every line), and its ink's left and right edges across every line from
    /// `text.bounds[0]` (None without a glyph), shaped exactly as `prepare` shapes it for an output `output_width`
    /// pixels wide.
    pub fn laid_out_extent(
        &mut self,
        fs: &mut FontSystem,
        output_width: u32,
        text: &PreparedText,
    ) -> (f32, Option<(f32, f32)>) {
        let (attrs, align, metrics, wrap_width, origin_dx) =
            shaping(text, output_width.max(1) as f32);
        let index = self.shape(
            fs,
            &text.content,
            &attrs,
            align,
            metrics,
            wrap_width,
            None,
            &text.runs,
        );
        let buffer = self.buffers.remove(index);
        let ink = buffer.layout_runs().filter_map(|run| run_ink(&run)).fold(
            None,
            |extent: Option<(f32, f32)>, (lo, hi)| {
                let (min, max) = extent.unwrap_or((lo, hi));
                Some((min.min(lo), max.max(hi)))
            },
        );
        (
            buffer.layout_runs().count() as f32 * metrics.line_height,
            ink.map(|(lo, hi)| (lo - origin_dx, hi - origin_dx)),
        )
    }

    /// Klipolo: makes the text-box pipeline now (it is otherwise made by the first text with a box), so the shader
    /// compiler runs during the warm-up instead of in a frame.
    pub fn warm_background(&mut self, device: &Device) {
        self.background
            .get_or_insert_with(|| TextBackgroundResources::new(device));
    }

    /// Klipolo: forgets every glyph rasterised so far, for a new font system (it numbers its faces anew, and glyphon and
    /// swash key glyphs by face id, so a kept glyph could be drawn for another face). The pipelines stay (`cache`).
    pub fn reset_atlas(&mut self, device: &Device, queue: &Queue) {
        self.resets += 1;
        self.swash_cache = SwashCache::new();
        self.text_atlas = TextAtlas::with_color_mode(
            device,
            queue,
            &self.cache,
            wgpu::TextureFormat::Rgba8Unorm,
            ColorMode::Web,
        );
        self.text_renderer = TextRenderer::new(
            &mut self.text_atlas,
            device,
            wgpu::MultisampleState::default(),
            None,
        );
        self.segment_renderers.clear();
        self.draws.clear();
        self.buffers.clear();
        self.outlines.clear();
        self.custom.clear();
        self.atlas_area = 0;
    }

    /// Klipolo: the text areas the last prepare drew (each stamped copy of an outline, shadow or glow is one), the work an
    /// outline costs.
    pub fn areas_last_prepare(&self) -> usize {
        self.areas
    }

    /// Klipolo: how many times the glyph atlas was made anew (`reset_atlas`).
    pub fn atlas_resets(&self) -> u64 {
        self.resets
    }

    /// Klipolo: outline glyphs made since the atlas was last made anew, and glyphs stroked in all.
    pub fn outline_counts(&self) -> (usize, u64) {
        self.outlines.counts()
    }

    /// Klipolo: evicts glyphs no prepare used since the last trim, so the atlas stays the size one text needs instead
    /// of growing with every size and string ever drawn.
    pub fn trim_atlas(&mut self) {
        self.text_atlas.trim();
    }

    /// Klipolo: glyphon's atlas only grows (each side doubles when a prepare's glyphs don't
    /// fit; trimming frees glyphs, never texture). After a text whose glyphs needed more than a `GLYPH_ATLAS_SIDE`²
    /// atlas holds (estimated from its distinct glyphs, `atlas_area`), it is made anew at its smallest, so it never
    /// stays larger than the cap after the text that grew it. Call once the text's drawing is submitted. Whether it
    /// was made anew.
    pub fn bound_atlas(&mut self, device: &Device, queue: &Queue) -> bool {
        if self.atlas_area <= GLYPH_ATLAS_AREA {
            return false;
        }
        self.reset_atlas(device, queue);
        true
    }

    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        fs: &mut FontSystem,
        output_size: (u32, u32),
        texts: &[PreparedText],
    ) {
        self.prepare_with_mode(device, queue, fs, output_size, texts, false);
    }

    pub fn prepare_mixed(
        &mut self,
        device: &Device,
        queue: &Queue,
        fs: &mut FontSystem,
        output_size: (u32, u32),
        texts: &[PreparedText],
    ) {
        self.prepare_with_mode(device, queue, fs, output_size, texts, true);
    }

    #[allow(clippy::too_many_arguments)]
    fn shape(
        &mut self,
        fs: &mut FontSystem,
        content: &str,
        attrs: &Attrs<'_>,
        align: Align,
        metrics: Metrics,
        wrap_width: f32,
        colors: Option<&[Color]>,
        runs: &Runs,
    ) -> usize {
        let mut buffer = Buffer::new(fs, metrics);
        // The box only constrains wrapping; height is unbounded so every
        // line is laid out even when the configured box is a little
        // shorter than the shaped text (e.g. font metric differences
        // between the editor's measurement and cosmic-text).
        buffer.set_size(fs, Some(wrap_width), None);
        // Klipolo: a word wider than the wrap width breaks between its letters, as the canvas editor and the webview's
        // measure break it (`overflow-wrap: break-word`), instead of running past the box and being cut there.
        buffer.set_wrap(fs, glyphon::Wrap::WordOrGlyph);
        if colors.is_none() && runs.is_empty() {
            buffer.set_text(fs, content, attrs, Shaping::Advanced);
            for line in buffer.lines.iter_mut() {
                line.set_align(Some(align));
            }
        } else {
            // Klipolo: one span per colour and run; a run asks for its own family at the text's weight and style.
            let spans = styled_spans(content, colors, runs);
            buffer.set_rich_text(
                fs,
                spans.iter().map(|(range, color, run)| {
                    let mut span = attrs.clone();
                    if let Some(color) = color {
                        span = span.color(*color);
                    }
                    if let Some(run) = run {
                        span = span.family(Family::Name(&runs[*run].1));
                    }
                    (&content[range.clone()], span)
                }),
                attrs,
                Shaping::Advanced,
                Some(align),
            );
        }
        buffer.shape_until_scroll(fs, false);
        self.buffers.push(buffer);
        self.buffers.len() - 1
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_with_mode(
        &mut self,
        device: &Device,
        queue: &Queue,
        fs: &mut FontSystem,
        output_size: (u32, u32),
        texts: &[PreparedText],
        force_segmented: bool,
    ) {
        // Klipolo: outline glyph ids used up, the atlas is made anew first (glyphon keeps custom glyphs by id).
        if self.outlines.full() {
            self.reset_atlas(device, queue);
        }
        self.outlines.begin();
        self.custom.clear();
        self.buffers.clear();
        self.draws.clear();
        let mut backgrounds: Vec<TextBackgroundUniforms> = Vec::new();
        let output_px = [output_size.0.max(1) as f32, output_size.1.max(1) as f32];

        for text in texts {
            let alpha = text.color[3].clamp(0.0, 1.0) * text.opacity.clamp(0.0, 1.0);

            let width = (text.bounds[2] - text.bounds[0]).max(1.0);
            let height = (text.bounds[3] - text.bounds[1]).max(1.0);

            let (attrs, align, metrics, wrap_width, origin_dx) = shaping(text, output_px[0]);

            let content = text.content.as_str();
            let plain = self.shape(
                fs, content, &attrs, align, metrics, wrap_width, None, &text.runs,
            );
            let laid_out_height =
                self.buffers[plain].layout_runs().count() as f32 * metrics.line_height;
            // The glyphs' own horizontal extent across every line, so a
            // background hugs the text rather than the authored box and a
            // wipe reveals the ink proportionally whatever the alignment.
            let ink = self.buffers[plain]
                .layout_runs()
                .filter_map(|run| run_ink(&run))
                .fold(None, |extent: Option<(f32, f32)>, (lo, hi)| {
                    let (min, max) = extent.unwrap_or((lo, hi));
                    Some((min.min(lo), max.max(hi)))
                });

            let staggered = text.stagger_in.is_some_and(|edge| edge.progress < 1.0)
                || text.stagger_out.is_some_and(|edge| edge.progress < 1.0);
            let mut unit_alpha = vec![1.0f32; content.len()];
            if staggered {
                stagger_alpha_by_byte(content, text.stagger_in, &mut unit_alpha);
                stagger_alpha_by_byte(content, text.stagger_out, &mut unit_alpha);
            }
            let mut gradient = text
                .gradient_color
                .map(|to| gradient_by_byte(&self.buffers[plain], content, text.color, to));
            // Klipolo: Karaoke's word in its own colour, over the text's (or its gradient's) colours.
            if let Some((range, color)) = &text.highlight {
                let colors = gradient.get_or_insert_with(|| vec![text.color; content.len()]);
                let end = range.end.min(colors.len());
                let start = range.start.min(end);
                colors[start..end].fill(*color);
            }

            // A pass whose colour varies per glyph needs its own buffer; a
            // flat pass re-tints the plain layout through default_color.
            let pass = |this: &mut Self,
                        fs: &mut FontSystem,
                        rgb: [f32; 4],
                        pass_alpha: f32,
                        offsets: Vec<[f32; 2]>,
                        is_main: bool| {
                let gradient = if is_main { gradient.as_deref() } else { None };
                if !staggered && gradient.is_none() {
                    return Pass {
                        buffer: plain,
                        color: to_color(rgb, pass_alpha),
                        offsets,
                        outline: None,
                    };
                }
                let colors: Vec<Color> = (0..content.len())
                    .map(|byte| {
                        let rgb = gradient.map_or(rgb, |colors| colors[byte]);
                        to_color(rgb, pass_alpha * unit_alpha[byte])
                    })
                    .collect();
                Pass {
                    buffer: this.shape(
                        fs,
                        content,
                        &attrs,
                        align,
                        metrics,
                        wrap_width,
                        Some(&colors),
                        &text.runs,
                    ),
                    color: to_color(rgb, pass_alpha),
                    offsets,
                    outline: None,
                }
            };

            // Animation transform: uniform scale about the box center plus a
            // translation, applied to the buffer origin and clip bounds (the
            // glyph layout itself is scaled by TextArea::scale from that
            // origin).
            let cx = (text.bounds[0] + text.bounds[2]) / 2.0;
            let cy = (text.bounds[1] + text.bounds[3]) / 2.0;
            let scale = text.scale.max(0.01);
            let tx = |x: f32| cx + (x - cx) * scale + text.offset[0];
            let ty = |y: f32| cy + (y - cy) * scale + text.offset[1];

            let block_left = text.bounds[0] - origin_dx;
            let origin_left = tx(block_left);
            let origin_top = ty(text.bounds[1]);
            let wipe = text.wipe.clamp(0.0, 1.0);
            let clip_right = match ink {
                Some((ink_left, ink_right)) if wipe < 1.0 => {
                    block_left + ink_left + (ink_right - ink_left) * wipe
                }
                _ => block_left + wrap_width,
            };

            // Clip horizontally at the (slack-expanded) wrap box, but extend
            // the bottom to the laid-out text height so descenders and extra
            // lines never get cut off; glyphon intersects these bounds with
            // the viewport. A wipe narrows the right edge.
            let bounds = TextBounds {
                left: origin_left.floor() as i32,
                top: origin_top.floor() as i32,
                right: tx(clip_right).ceil() as i32,
                bottom: ty(text.bounds[1] + height.max(laid_out_height)).ceil() as i32,
            };

            // Klipolo: an outline pass draws its buffer's letters grown by the outline, one glyph each
            // (`outline_glyphs`), where Cap stamped the letters on rings out to it (up to 196 copies).
            let outlined = |this: &mut Self,
                            fs: &mut FontSystem,
                            mut p: Pass,
                            [dx, dy]: [f32; 2],
                            width: f32| {
                p.outline = Some(outline_glyphs(
                    &mut this.outlines,
                    &mut this.custom,
                    fs,
                    &this.buffers[p.buffer],
                    origin_left + dx,
                    origin_top + dy,
                    scale,
                    width,
                ));
                p
            };

            let mut passes = Vec::new();
            if text.shadow > 0.0 {
                // Klipolo: an outlined shadow moves a whole pixel across, so its letters keep their
                // subpixel bins (cosmic-text already truncates them down) and its outline glyphs are the outline's own:
                // each letter is stroked once, not twice.
                let dx = text.font_size * scale * 0.02;
                let dx = if text.stroke.is_some() {
                    dx.round()
                } else {
                    dx
                };
                let dy = text.font_size * scale * 0.055;
                let shadow_alpha = alpha * text.shadow.clamp(0.0, 1.0) * 0.85;
                let shadow = pass(
                    self,
                    fs,
                    [0.0, 0.0, 0.0, 1.0],
                    shadow_alpha,
                    vec![[dx, dy]],
                    false,
                );
                // With an outline the shadow falls from the outlined letters (otherwise the outline covers it).
                passes.push(match text.stroke {
                    Some((stroke_px, _)) => outlined(self, fs, shadow, [dx, dy], stroke_px),
                    None => shadow,
                });
            }
            if text.glow > 0.0 {
                for (radius_em, ring_alpha) in GLOW_RINGS {
                    let radius = radius_em * text.font_size * scale;
                    passes.push(pass(
                        self,
                        fs,
                        text.color,
                        alpha * ring_alpha * text.glow,
                        ring(radius).collect(),
                        false,
                    ));
                }
            }
            if let Some((stroke_px, stroke_color)) = text.stroke {
                let outline = pass(self, fs, stroke_color, alpha, vec![[0.0, 0.0]], false);
                passes.push(outlined(self, fs, outline, [0.0, 0.0], stroke_px));
            }
            passes.push(pass(self, fs, text.color, alpha, vec![[0.0, 0.0]], true));

            let background_start = backgrounds.len();
            if let Some(background_color) = text.background_color {
                let color = [
                    background_color[0],
                    background_color[1],
                    background_color[2],
                    background_color[3] * text.opacity.clamp(0.0, 1.0),
                ];
                let uniforms = |(mut rect, radius): ([f32; 4], f32)| {
                    rect[2] *= wipe;
                    TextBackgroundUniforms {
                        rect,
                        color,
                        radius: radius.min(rect[2] * 0.5),
                        _padding0: 0.0,
                        _padding1: 0.0,
                        _padding2: 0.0,
                        output_size: output_px,
                        _padding3: [0.0; 2],
                    }
                };
                match text.background_style {
                    TextBackgroundStyle::Highlight => {
                        for run in self.buffers[plain].layout_runs() {
                            let Some((ink_left, ink_right)) = run_ink(&run) else {
                                continue;
                            };
                            backgrounds.push(uniforms(
                                crate::vendor::cap::rendering::text::highlight_rect(
                                    (block_left + ink_left, block_left + ink_right),
                                    (text.bounds[1] + run.line_top, run.line_height),
                                    text.font_size,
                                    [cx, cy],
                                    scale,
                                    text.offset,
                                ),
                            ));
                        }
                    }
                    style => {
                        let hugged = ink.map_or(text.bounds, |(ink_left, ink_right)| {
                            [
                                block_left + ink_left,
                                text.bounds[1],
                                block_left + ink_right,
                                text.bounds[3],
                            ]
                        });
                        backgrounds.push(uniforms(
                            crate::vendor::cap::rendering::text::background_rect(
                                hugged,
                                laid_out_height,
                                text.font_size,
                                [cx, cy],
                                scale,
                                text.offset,
                                style == TextBackgroundStyle::Pill,
                            ),
                        ));
                    }
                }
            }

            self.draws.push(TextDraw {
                track: text.track,
                bounds,
                left: origin_left,
                top: origin_top,
                scale,
                passes,
                background: background_start..backgrounds.len(),
            });
        }

        self.areas = self
            .draws
            .iter()
            .flat_map(|draw| &draw.passes)
            .map(|pass| pass.offsets.len())
            .sum();
        self.atlas_area = atlas_area(
            &self.buffers,
            &self.custom,
            self.draws.first().map_or(1.0, |d| d.scale),
            &mut self.seen,
        );
        self.viewport.update(
            queue,
            Resolution {
                width: output_size.0,
                height: output_size.1,
            },
        );

        self.segmented_render = force_segmented || !backgrounds.is_empty();
        if self.segmented_render {
            let background = self
                .background
                .get_or_insert_with(|| TextBackgroundResources::new(device));
            background.ensure_capacity(device, backgrounds.len());
            for (index, uniforms) in backgrounds.iter().enumerate() {
                background.write(queue, index, uniforms);
            }

            while self.segment_renderers.len() < self.draws.len() {
                self.segment_renderers.push(TextRenderer::new(
                    &mut self.text_atlas,
                    device,
                    wgpu::MultisampleState::default(),
                    None,
                ));
            }
            for (index, draw) in self.draws.iter().enumerate() {
                if let Err(error) = self.segment_renderers[index].prepare_with_custom(
                    device,
                    queue,
                    fs,
                    &mut self.text_atlas,
                    &self.viewport,
                    text_areas(&self.buffers, &self.empty, &self.custom, draw),
                    &mut self.swash_cache,
                    |ask| self.outlines.image(ask),
                ) {
                    warn!("Failed to prepare text: {error:?}");
                }
            }
        } else if let Err(error) = self.text_renderer.prepare_with_custom(
            device,
            queue,
            fs,
            &mut self.text_atlas,
            &self.viewport,
            self.draws
                .iter()
                .flat_map(|draw| text_areas(&self.buffers, &self.empty, &self.custom, draw)),
            &mut self.swash_cache,
            |ask| self.outlines.image(ask),
        ) {
            warn!("Failed to prepare text: {error:?}");
        }
    }

    fn render_draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, index: usize) {
        let draw = &self.draws[index];
        if let Some(background) = &self.background {
            for rect in draw.background.clone() {
                background.render(pass, rect);
            }
        }
        if let Err(error) =
            self.segment_renderers[index].render(&self.text_atlas, &self.viewport, pass)
        {
            warn!("Failed to render text: {error:?}");
        }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.segmented_render {
            for index in 0..self.draws.len() {
                self.render_draw(pass, index);
            }
        } else if let Err(error) = self
            .text_renderer
            .render(&self.text_atlas, &self.viewport, pass)
        {
            warn!("Failed to render text: {error:?}");
        }
    }

    pub fn render_track<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, track: u32) {
        if !self.segmented_render {
            return;
        }
        for index in 0..self.draws.len() {
            if self.draws[index].track == track {
                self.render_draw(pass, index);
            }
        }
    }

    pub fn has_track(&self, track: u32) -> bool {
        self.segmented_render && self.draws.iter().any(|draw| draw.track == track)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_letters_map_bytes_to_reveal_units() {
        let (map, units) = unit_map("Hi there", true);
        assert_eq!(units, 2);
        assert_eq!(map, [0, 0, 0, 1, 1, 1, 1, 1]);

        let (map, units) = unit_map("Hi there", false);
        assert_eq!(units, 7);
        assert_eq!(map, [0, 1, 1, 2, 3, 4, 5, 6]);

        let (map, units) = unit_map("é a", false);
        assert_eq!(units, 2);
        assert_eq!(map, [0, 0, 0, 1]);
    }

    #[test]
    fn styled_spans_rebuild_the_whole_string() {
        let content = "ab\ncd";
        let red = Color::rgb(255, 0, 0);
        let blue = Color::rgb(0, 0, 255);
        let colors = [red, red, red, blue, blue];
        let spans = styled_spans(content, Some(&colors), &[]);
        assert_eq!(
            spans,
            vec![(0..3, Some(red), None), (3..5, Some(blue), None)]
        );
        let rebuilt: String = spans
            .iter()
            .map(|(range, _, _)| &content[range.clone()])
            .collect();
        assert_eq!(rebuilt, content);
    }

    #[test]
    fn styled_spans_split_at_runs_and_colours_alike() {
        // "Hi 東京": Latin in the text's face, the kanji in a CJK fallback, the word being said ("i 東") in yellow.
        let content = "Hi 東京";
        let (white, yellow) = (Color::rgb(255, 255, 255), Color::rgb(255, 212, 0));
        let mut colors = vec![white; content.len()];
        colors[1..6].fill(yellow);
        let runs: Vec<(Range<usize>, Arc<str>)> =
            vec![(0..3, "kl:b:inter".into()), (3..9, "kl:fallback:jp".into())];
        assert_eq!(
            styled_spans(content, Some(&colors), &runs),
            vec![
                (0..1, Some(white), Some(0)),
                (1..3, Some(yellow), Some(0)),
                (3..6, Some(yellow), Some(1)),
                (6..9, Some(white), Some(1)),
            ]
        );
        assert_eq!(
            styled_spans(content, None, &runs),
            vec![(0..3, None, Some(0)), (3..9, None, Some(1))]
        );
    }

    #[test]
    fn line_starts_follow_newlines() {
        assert_eq!(line_starts("a\nbc\n"), [0, 2, 5]);
    }
}
