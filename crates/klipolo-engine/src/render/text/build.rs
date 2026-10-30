//! What builds a title (split from `render/text.rs` with no change to what is drawn): shaping it
//! (`lay_out`) and drawing a window of its layout into a texture (`draw`), with the vendored glyph layer, the faces it
//! resolved and the mipmap generator. The render thread keeps one for a title no one built ahead (a cold seek, an
//! export's frames); the title build thread keeps its own (`worker`), so the lookahead's titles are shaped, rasterised
//! and mipmapped there.

use std::sync::Arc;

use klipolo_model::font::FontSource;
use klipolo_model::timeline::{TextAlign as Align, TextStyle};

use super::faces::{Faces, Notes, runs_and_notes};
use super::geometry::{Window, padding};
use super::{LAYOUTS, LINE_HEIGHT, SHADOW, wrap};
use crate::plan::TextLayer;
use crate::plan::color::parse_hex;
use crate::render::mips::{MipGenerator, level_count};
use crate::render::shared_fonts::SharedFonts;
use crate::render::text_timing::Stamps;
use crate::vendor::cap::rendering::layers::text::TextLayer as GlyphLayer;
use crate::vendor::cap::rendering::text::{PreparedText, TextAlign, TextBackgroundStyle};

/// What a title's layout depends on: its words, style and sizes in texture pixels, its language, and the margin its
/// padded filters draw past it.
#[derive(Clone, PartialEq)]
pub(super) struct Key {
    pub(super) text: Arc<str>,
    pub(super) style: Arc<TextStyle>,
    pub(super) font_px: u32,
    pub(super) box_px: (u32, u32),
    /// The text language its Chinese, Japanese and Korean follow.
    pub(super) language: Arc<str>,
    /// `TextLayer::pad_px`, bit for bit: a filter's margin changing lays the text out again in a texture with room for
    /// it.
    pub(super) pad: u64,
}

impl Key {
    /// Whether `layer` lays out as this key did.
    pub(super) fn matches(&self, layer: &TextLayer, font_px: u32, box_px: (u32, u32)) -> bool {
        self.font_px == font_px && self.box_px == box_px && self.same_text(layer)
    }

    /// Whether `layer` says what this key lays out, at whatever size: its text, its style (by value), its language and
    /// its filters' margin (1080p pixels, not a size).
    pub(super) fn same_text(&self, layer: &TextLayer) -> bool {
        self.pad == layer.pad_px.to_bits()
            && self.text == layer.text
            && (Arc::ptr_eq(&self.style, &layer.style) || self.style == layer.style)
            && self.language == layer.language
    }
}

/// A text laid out in its whole texture (its box and padding, before any window or cap), in texture pixels.
#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub(super) full: (u32, u32),
    /// The width it is shaped against (`wrap::shaped`), the same for every window of it drawn.
    pub(super) layout_width: u32,
    /// The text block's left and top edges and its laid-out height.
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) laid_out: f32,
    pub(super) stroke: f32,
    /// What it tells each frame that draws it (a stand-in font, CJK without its Noto Sans).
    pub(super) notes: Notes,
    /// The part of the whole a texture of it needs: its ink and the room its outline,
    /// shadow, background and overhang take round it, not the whole box; None when it has no glyph (the whole is
    /// drawn, as a background then fills the box).
    pub(super) ink: Option<Window>,
}

/// See the module doc.
pub(super) struct TitleBuilder {
    glyphs: GlyphLayer,
    /// The faces resolved so far (cleared whenever the fonts change).
    faces: Faces,
    /// The fonts changed: the glyph layer forgets its glyphs before the next text (a new font system numbers its faces
    /// anew, and glyphs are kept by face id).
    stale_glyphs: bool,
    /// Mipmaps for texts laid out bigger than they are drawn; made with the first one.
    mips: Option<Arc<MipGenerator>>,
    /// Layouts and draws made (`TitleStats::built_here` for the render thread's builder).
    builds: u64,
    /// The perf harness's GPU timestamps: the render thread's builder writes a pair round each draw.
    stamps: Option<Arc<Stamps>>,
    /// The last draw's submission (the title build thread waits for it before handing the texture over).
    submitted: Option<wgpu::SubmissionIndex>,
}

impl TitleBuilder {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            glyphs: GlyphLayer::new(device, queue),
            faces: Faces::default(),
            stale_glyphs: false,
            mips: None,
            builds: 0,
            stamps: None,
            submitted: None,
        }
    }

    /// The last draw's submission, once.
    pub(super) fn take_submission(&mut self) -> Option<wgpu::SubmissionIndex> {
        self.submitted.take()
    }

    /// Times each draw's GPU work into `stamps` (the perf harness), or stops.
    pub(super) fn time_gpu(&mut self, stamps: Option<Arc<Stamps>>) {
        self.stamps = stamps;
    }

    /// Makes the pipelines a text can need (glyphs are made in `new`; text boxes on first use) ahead of time.
    pub(super) fn warm(&mut self, device: &wgpu::Device) {
        self.glyphs.warm_background(device);
    }

    /// The fonts changed: faces are resolved again and glyphs rasterised again (before the next text).
    pub(super) fn fonts_changed(&mut self) {
        self.stale_glyphs = true;
        self.faces.clear();
    }

    /// Before a text: the glyphs of fonts that changed go.
    pub(super) fn begin(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if std::mem::take(&mut self.stale_glyphs) {
            self.glyphs.reset_atlas(device, queue);
        }
    }

    pub(super) fn builds(&self) -> u64 {
        self.builds
    }

    pub(super) fn glyphs(&self) -> &GlyphLayer {
        &self.glyphs
    }

    /// The text's whole texture: its box plus room for the background, outline, shadow and the wrap slack, and the
    /// `margin_px` its padded filters draw past it, taller when the text needs more lines than the box holds
    /// (centred overflow), and where the text block sits in it. Shapes the text (cosmic-text): only when the text, its
    /// style, its size in pixels or its margin changes.
    pub(super) fn lay_out(
        &mut self,
        fonts: &mut SharedFonts,
        key: &Key,
        stroke_px: f32,
        margin_px: f32,
    ) -> Layout {
        LAYOUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.builds += 1;
        let (box_w, box_h) = (key.box_px.0 as f32, key.box_px.1 as f32);
        // Wrapped against the width its texture has whole (capped as before), whatever window is drawn: so always at
        // box × 1.05 + 4, as the canvas editor wraps (`wrap`).
        let wrap::Shaped {
            pad: (pad_x, pad_y),
            full_w,
            layout_width,
        } = wrap::shaped(box_w, key.font_px as f32, stroke_px, margin_px);
        let (mut text, notes) = self.prepared(key, stroke_px, fonts.scanned());
        text.bounds = [pad_x, pad_y, pad_x + box_w, pad_y + box_h];
        text.layout_width = Some(layout_width as f32);
        let (laid_out, ink) = self
            .glyphs
            .laid_out_extent(fonts.system(), layout_width, &text);
        let content = box_h.max(laid_out);
        let full = (
            full_w.max(1),
            ((content + 2.0 * pad_y).ceil() as u32).max(1),
        );
        // The block (its laid-out height, so a background hugs the text) centred in the box.
        let top = pad_y + (content - laid_out) / 2.0;
        // Round the ink: the outline, the shadow (0.055 em down), a background (0.2 em) and ink past the advances, as
        // the box's own padding allows for (`geometry::padding`, without the wrap slack), and the margin its padded
        // filters draw into (the texture keeps it, so they need no extra pass).
        let (room, _) = padding(f64::from(key.font_px), f64::from(stroke_px), 0.0);
        let room = room as f32 + margin_px;
        let ink = ink.and_then(|(lo, hi)| {
            let (x0, x1) = (
                (pad_x + lo - room).floor().max(0.0) as u32,
                ((pad_x + hi + room).ceil() as u32).min(full.0),
            );
            let (y0, y1) = (
                (top - room).floor().max(0.0) as u32,
                ((top + laid_out + room).ceil() as u32).min(full.1),
            );
            (x1 > x0 && y1 > y0).then_some(Window {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            })
        });
        Layout {
            full,
            layout_width,
            left: pad_x,
            top,
            laid_out,
            stroke: stroke_px,
            notes,
            ink,
        }
    }

    /// `key`'s text as the glyph layer takes it, its bounds still to set, and what it tells the frames that draw it.
    /// It asks for exactly the face the resolver picked, at that face's own weight and style (cosmic-text matches
    /// exact weights only), and its CJK stretches for their language's face.
    fn prepared(&mut self, key: &Key, stroke_px: f32, scanned: bool) -> (PreparedText, Notes) {
        let style = &key.style;
        let choice = self.faces.choice(style);
        let scanning = !scanned && style.font.source == FontSource::Installed;
        let (runs, notes) = runs_and_notes(&key.text, &choice, &key.language, scanning);
        // A broken install's stand-in has no file: the system sans-serif.
        let family = if choice.no_file {
            "sans-serif".to_string()
        } else {
            choice.family
        };
        let [r, g, b] = parse_hex(&style.color);
        let text = PreparedText {
            track: 0,
            content: key.text.to_string(),
            bounds: [0.0; 4],
            color: [unit(r), unit(g), unit(b), 1.0],
            background_color: style.background.as_deref().map(|hex| {
                let [r, g, b] = parse_hex(hex);
                [unit(r), unit(g), unit(b), 1.0]
            }),
            font_family: family,
            font_size: key.font_px as f32,
            font_weight: f32::from(choice.weight),
            italic: choice.italic,
            stretch: choice.stretch,
            opacity: 1.0,
            align: match style.align {
                Align::Left => TextAlign::Left,
                Align::Center => TextAlign::Center,
                Align::Right => TextAlign::Right,
            },
            letter_spacing: 0.0,
            line_height: LINE_HEIGHT,
            shadow: if style.shadow { SHADOW } else { 0.0 },
            offset: [0.0, 0.0],
            scale: 1.0,
            stroke: style.outline.as_ref().filter(|_| stroke_px > 0.0).map(|o| {
                let [r, g, b] = parse_hex(&o.color);
                (stroke_px, [unit(r), unit(g), unit(b), 1.0])
            }),
            glow: 0.0,
            gradient_color: None,
            background_style: TextBackgroundStyle::Box,
            wipe: 1.0,
            stagger_in: None,
            stagger_out: None,
            layout_width: None,
            highlight: None,
            runs,
        };
        (text, notes)
    }

    /// Draws `window` of the text laid out as `layout` into a texture of the window's size, and submits it. `mipmapped`:
    /// it is laid out bigger than it is drawn at rest, so it gets mipmaps to be drawn smaller without shimmering.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fonts: &mut SharedFonts,
        key: &Key,
        layout: &Layout,
        window: Window,
        mipmapped: bool,
    ) -> wgpu::Texture {
        self.builds += 1;
        let (width, height) = (window.w.max(1), window.h.max(1));
        let (mut text, _) = self.prepared(key, layout.stroke, fonts.scanned());
        text.layout_width = Some(layout.layout_width as f32);
        let (left, top) = (layout.left - window.x as f32, layout.top - window.y as f32);
        text.bounds = [
            left,
            top,
            left + key.box_px.0 as f32,
            top + layout.laid_out.max(1.0),
        ];

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Text layer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: if mipmapped {
                level_count(width, height)
            } else {
                1
            },
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        // The glyphs go into level 0 (a render target is one level).
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            mip_level_count: Some(1),
            ..Default::default()
        });
        self.glyphs.prepare(
            device,
            queue,
            fonts.system(),
            (width, height),
            std::slice::from_ref(&text),
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Text layer"),
        });
        let stamps = self
            .stamps
            .as_ref()
            .and_then(|stamps| Some((stamps, stamps.pair()?)));
        if let Some((stamps, (start, _))) = stamps {
            encoder.write_timestamp(stamps.set(), start);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Text layer"),
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
            self.glyphs.render(&mut pass);
        }
        if mipmapped {
            self.mips
                .get_or_insert_with(|| MipGenerator::new(device))
                .encode(device, &mut encoder, &texture);
        }
        if let Some((stamps, (_, end))) = stamps {
            encoder.write_timestamp(stamps.set(), end);
        }
        // Submitted now, mipmaps with the glyphs: the glyph layer's buffers are rewritten for the next text, and this
        // texture is sampled by a frame's own submission, which comes later on the same queue (the build thread's
        // included: it hands the texture over only after this returns).
        self.submitted = Some(queue.submit([encoder.finish()]));
        self.glyphs.trim_atlas();
        // The glyph atlas never stays past its cap after a text that grew it.
        self.glyphs.bound_atlas(device, queue);
        texture
    }
}

fn unit(c: u8) -> f32 {
    f32::from(c) / 255.0
}
