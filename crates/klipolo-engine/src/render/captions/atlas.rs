//! A caption page's word atlas: each word drawn once into its own cell (`layout::WordBox::cell`) as four packed masks:
//! R the fill, G the silhouette (the fill with its outline), B the silhouette blurred for the shadow, A the fill
//! blurred for the glow. The drawer draws the page's words from it with instanced quads, colouring each mask per word
//! and per moment; the atlas itself is built once per page, look or font change, never per frame.
//!
//! The words are drawn with the vendored glyph layer at their laid-out size (white, the fill on one target and the
//! outlined silhouette on another, in one prepare), then `captions.wgsl` packs the two and blurs them across and down
//! with the taps `blur` works out (at half size for wide blurs, then upscaled into the atlas).
//! Memory: the atlas is `size.0 × size.1 × 4` bytes with no mipmaps (it draws at 1:1 or smaller); building it takes
//! two scratch targets of its size for the length of one submission (a peak of 3× the atlas), and two of a quarter its
//! size more for a half-size blur (3.5×), freed when it is done.

use std::cell::Cell;

use glyphon::FontSystem;
use klipolo_model::captions::Pages;
use klipolo_model::captions::style::CaptionStyleDef;
use klipolo_model::captions::styles::CaptionStyles;
use klipolo_model::contrib::ContribId;
use klipolo_model::font::FontRef;
use klipolo_model::project::{CaptionSettings, FormatKind};

use super::blur::{Uniform, blurs};
use super::cells::blur_sigmas;
use super::layout::{PageLayout, WordBox, lay_out_page};
use super::pipes::{FORMAT, Pipes, pass};
use super::shaping::joins_letters;
use crate::render::fonts::{self, family_for};
use crate::vendor::cap::rendering::layers::text::TextLayer as GlyphLayer;
use crate::vendor::cap::rendering::text::{PreparedText, TextAlign, TextBackgroundStyle};

thread_local! {
    /// Tests: this thread draws frames and must never build a page, nor free one's atlas
    /// (`forbid_builds_on_this_thread`).
    static NO_BUILDS: Cell<bool> = const { Cell::new(false) };
}

/// Tests: a page built, or a page's atlas freed, on this thread from now on panics, so a test proves the render thread
/// does neither (pages are built, and their atlases freed, on the caption build thread).
#[doc(hidden)]
pub fn forbid_builds_on_this_thread() {
    NO_BUILDS.set(true);
}

/// One page's word atlas.
pub struct PageAtlas {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
    pub bytes: u64,
}

impl Drop for PageAtlas {
    fn drop(&mut self) {
        assert!(
            !NO_BUILDS.get() || std::thread::panicking(),
            "a caption page's atlas was freed on a thread that draws frames"
        );
    }
}

/// What builds atlases: the glyph layer the words are drawn with and the pack and blur pipelines, made once (the
/// caption drawer keeps one).
pub struct AtlasBuilder {
    glyphs: GlyphLayer,
    pipes: Pipes,
    /// The fonts changed (`fonts_changed`): the glyph layer forgets its glyphs before the next build.
    stale_glyphs: bool,
}

impl AtlasBuilder {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            glyphs: GlyphLayer::new(device, queue),
            pipes: Pipes::new(device),
            stale_glyphs: false,
        }
    }

    /// The glyph layer's text areas in the last page built (each stamped copy is one), for the outline test.
    #[doc(hidden)]
    pub fn glyph_areas(&self) -> usize {
        self.glyphs.areas_last_prepare()
    }

    /// The shared fonts changed (`SharedFonts::refresh` returned true): a new font system numbers its faces anew, and
    /// glyphs are kept by face id, so the next build rasterises its glyphs again.
    pub fn fonts_changed(&mut self) {
        self.stale_glyphs = true;
    }

    /// Builds one tiny page in the default style with `fs` (the build thread's font system), so the glyph layer's
    /// renderers and the pack and blur passes have run once before the first real page (a first build
    /// took about 200 ms on lavapipe).
    pub fn warm(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, fs: &mut FontSystem) {
        // A broken install (no bundled folder, and no system sans-serif to stand in) has nothing to shape with.
        if fs.db().is_empty() {
            return;
        }
        let settings = CaptionSettings {
            enabled: true,
            style: Some(ContribId::builtin("caption.bold")),
            ..CaptionSettings::default()
        };
        let format = FormatKind::Portrait;
        let style = CaptionStyles::builtin()
            .for_settings(&settings, &[format])
            .0;
        let font = &style.font;
        let face = family_for(
            &fonts::index(),
            &FontRef::bundled(&font.family),
            font.weight,
            font.italic,
        );
        let pages = Pages::one_page("warm", 0, 1_000_000, &[("Aa", 0, 500_000)]);
        let layout = lay_out_page(fs, &style, &face, "en", &pages, 0, format, (270, 480), 0.7);
        drop(self.build_atlas(device, queue, fs, &style, &layout));
    }

    /// The atlas of `layout`'s words (their text is the layout's own: `PageLayout::word_text`), in `style`.
    pub fn build_atlas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fs: &mut FontSystem,
        style: &CaptionStyleDef,
        layout: &PageLayout,
    ) -> PageAtlas {
        assert!(
            !NO_BUILDS.get(),
            "a caption page was built on a thread that draws frames"
        );
        if std::mem::take(&mut self.stale_glyphs) {
            self.glyphs.reset_atlas(device, queue);
        }
        let side = device.limits().max_texture_dimension_2d;
        let size = (layout.atlas.0.clamp(1, side), layout.atlas.1.clamp(1, side));
        let sized = |label: &str, (width, height): (u32, u32), extra: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | extra,
                view_formats: &[],
            })
        };
        let target = |label: &str, extra| sized(label, size, extra);
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let none = wgpu::TextureUsages::empty();
        // Scratch: the fill (once packed, the blur's first direction) and the outlined silhouette.
        // Dropped when this returns; wgpu frees them once the submission below is done with them.
        let (fill, outlined) = (
            target("Caption fill", none),
            target("Caption silhouette", none),
        );
        let atlas = target("Caption atlas", wgpu::TextureUsages::COPY_SRC);
        let (fill_v, outlined_v, atlas_v) = (view(&fill), view(&outlined), view(&atlas));

        // Every word twice in one prepare: the fill (track 0) and the outlined silhouette (track 1).
        let prepared: Vec<PreparedText> = layout
            .words
            .iter()
            .enumerate()
            .flat_map(|(k, w)| [false, true].map(|outline| word_text(style, layout, k, w, outline)))
            .collect();
        self.glyphs
            .prepare_mixed(device, queue, fs, size, &prepared);

        // The blurs at the page's size: σ is half the blur, in em (never capped in pixels); no glow keeps A empty.
        let em = layout.font_px * layout.autofit;
        let (shadow_sigma, glow_sigma) = blur_sigmas(style, em);
        let plan = blurs(shadow_sigma, glow_sigma, size);
        let pipes = &self.pipes;
        let packed = pipes.sources(device, &fill_v, &outlined_v, false);
        let pack_params = pipes.blur_params(device, &Uniform::pack(glow_sigma >= 0.0));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Caption atlas"),
        });
        self.glyphs
            .render_track(&mut pass(&mut encoder, &fill_v), 0);
        self.glyphs
            .render_track(&mut pass(&mut encoder, &outlined_v), 1);
        pipes.pack(&mut encoder, &atlas_v, &packed, &pack_params);
        // Each blur pass binds the silhouette as its unread second texture: it is never written there.
        match &plan {
            // Nothing to blur: the pack pass wrote every mask.
            None => {}
            // Across into the fill's target (free once packed), then down into the atlas.
            Some(b) if b.down == 1 => {
                let across_from = pipes.sources(device, &atlas_v, &outlined_v, true);
                let down_from = pipes.sources(device, &fill_v, &outlined_v, true);
                let across = pipes.blur_params(device, &Uniform::pass(b, [1.0, 0.0]));
                let down = pipes.blur_params(device, &Uniform::pass(b, [0.0, 1.0]));
                pipes.blur(&mut encoder, &fill_v, &across_from, &across);
                pipes.blur(&mut encoder, &atlas_v, &down_from, &down);
            }
            // Half size: the masks halved, blurred across and down there, and upscaled into the atlas's blurred masks.
            Some(b) => {
                let half = (size.0.div_ceil(2), size.1.div_ceil(2));
                let (one, two) = (
                    sized("Caption blur", half, none),
                    sized("Caption blur", half, none),
                );
                let (one_v, two_v) = (view(&one), view(&two));
                let halve_from = pipes.sources(device, &atlas_v, &outlined_v, true);
                let across_from = pipes.sources(device, &one_v, &outlined_v, true);
                let down_from = pipes.sources(device, &two_v, &outlined_v, true);
                let up_from = pipes.sources(device, &one_v, &outlined_v, true);
                let across = pipes.blur_params(device, &Uniform::pass(b, [1.0, 0.0]));
                let down = pipes.blur_params(device, &Uniform::pass(b, [0.0, 1.0]));
                pipes.halve(&mut encoder, &one_v, &halve_from);
                pipes.blur(&mut encoder, &two_v, &across_from, &across);
                pipes.blur(&mut encoder, &one_v, &down_from, &down);
                pipes.combine(
                    &mut encoder,
                    &atlas_v,
                    &up_from,
                    b.shadow_blurred,
                    b.glow_blurred,
                );
            }
        }
        queue.submit([encoder.finish()]);
        self.glyphs.trim_atlas();
        // The glyph atlas never stays past its cap after a page that grew it.
        self.glyphs.bound_atlas(device, queue);
        PageAtlas {
            view: atlas_v,
            texture: atlas,
            size,
            bytes: u64::from(size.0) * u64::from(size.1) * 4,
        }
    }
}

/// Word `k` as the glyph layer draws it into its cell: white, at its laid-out size and face, its box centred in the
/// cell; with `outline`, the style's outline around it (the silhouette). It never wraps: its wrap width is wider than
/// its box (`layout_width` set very wide), and a caption word is never broken.
fn word_text(
    style: &CaptionStyleDef,
    layout: &PageLayout,
    k: usize,
    w: &WordBox,
    outline: bool,
) -> PreparedText {
    let face = &layout.faces[usize::from(w.face)];
    let px = (layout.font_px * layout.autofit * w.scale).max(1.0);
    let [x, y, cw, ch] = w.cell;
    // The glyph layer clips a text to its wrap box (`width × 1.05 + 4`, centred on a centred text) and its line. Asked
    // for the width whose wrap box is the cell, with the cell as its one line, the word is drawn centred where it was
    // measured (its box is centred in the cell) and its ink may reach anywhere in the cell: an italic's or a script's
    // overhang past the advance is never cut, nor drawn into the next cell.
    let (cw, ch) = (cw as f32, ch as f32);
    let centre = x as f32 + cw / 2.0;
    let half = ((cw - 4.0) / 1.05).max(1.0) / 2.0;
    let stroke = style
        .outline
        .as_ref()
        .filter(|_| outline)
        .map(|o| (o.width_em as f32 * px, [1.0; 4]));
    PreparedText {
        track: u32::from(outline),
        content: layout.word_text(w).to_owned(),
        bounds: [centre - half, y as f32, centre + half, y as f32 + ch],
        color: [1.0; 4],
        background_color: None,
        font_family: if face.no_file {
            "sans-serif".to_owned()
        } else {
            face.family.clone()
        },
        font_size: px,
        font_weight: f32::from(face.weight),
        italic: face.italic,
        stretch: face.stretch,
        opacity: 1.0,
        align: TextAlign::Center,
        // None in a script that joins its letters, as the word was measured (`joins_letters`).
        letter_spacing: if joins_letters(layout.word_text(w)) {
            0.0
        } else {
            style.letter_spacing_em as f32 * px
        },
        // The line is the cell, so the glyphs sit centred in it as they were measured (the box is centred there too).
        line_height: ch.max(1.0) / px,
        shadow: 0.0,
        offset: [0.0, 0.0],
        scale: 1.0,
        stroke,
        glow: 0.0,
        gradient_color: None,
        background_style: TextBackgroundStyle::Box,
        wipe: 1.0,
        stagger_in: None,
        stagger_out: None,
        layout_width: Some(f32::MAX),
        highlight: None,
        runs: layout.runs_of(k),
    }
}
