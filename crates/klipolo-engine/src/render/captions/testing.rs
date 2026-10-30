//! Test helpers for the caption renderer (doc-hidden): a page's atlas built and read back.

use std::sync::Arc;

use klipolo_model::captions::Pages;
use klipolo_model::captions::styles::CaptionStyles;
use klipolo_model::contrib::ContribId;
use klipolo_model::font::FontRef;
use klipolo_model::project::{CaptionSettings, FormatKind};

use super::{atlas, layout};
use crate::render::device::RenderDevice;
use crate::render::fonts::{self, family_for};
use crate::render::shared_fonts::SharedFonts;

/// Tests: the font system the render thread has before the system font scan is done (the managed faces only).
#[doc(hidden)]
pub fn test_font_system() -> SharedFonts {
    SharedFonts::new(false)
}

/// Tests: a page's atlas read back.
#[doc(hidden)]
pub struct TestAtlas {
    pub cells: Vec<[u32; 4]>,
    pub size: (u32, u32),
    pub bytes: u64,
    /// How long the layout and the atlas took, until the GPU was done, ms: the first time (`cold`: the shaping, the
    /// glyphs and the glyph layer's renderers made), and again with the same builder and fonts (`build_ms`: the page
    /// after it in the same style).
    pub cold_ms: f64,
    pub build_ms: f64,
    pub lines: usize,
    /// Each word's box (its advance by its face's height), centred in its cell, and the page's em px (its size times
    /// its autofit).
    pub boxes: Vec<[f32; 2]>,
    pub em: f32,
    /// The glyph layer's text areas for the page (each stamped copy of an outline is one).
    pub areas: usize,
    pixels: Vec<u8>,
}

impl TestAtlas {
    /// The sum of channel `channel` (0 R … 3 A) over `cell`.
    pub fn channel_sum(&self, cell: [u32; 4], channel: usize) -> u64 {
        let [x, y, w, h] = cell;
        (y..y + h)
            .flat_map(|row| (x..x + w).map(move |col| (row, col)))
            .map(|(row, col)| {
                let at = ((row * self.size.0 + col) * 4) as usize + channel;
                u64::from(self.pixels[at])
            })
            .sum()
    }

    /// Channel `channel` at (`x`, `y`).
    pub fn at(&self, x: u32, y: u32, channel: usize) -> u8 {
        self.pixels[((y * self.size.0 + x) * 4) as usize + channel]
    }
}

/// Tests: one page of `words` (shown as given) in the built-in style `style` (`caption.bold`), laid out for a 1080 × 1920
/// portrait frame through `lay_out_page`, drawn by `build_atlas` and read back. Needs a GPU.
#[doc(hidden)]
pub async fn test_atlas(style: &str, words: &[&str]) -> TestAtlas {
    test_atlas_with(style, words, None, (1080, 1920)).await
}

/// Tests: `test_atlas` in the bundled `font` (family, weight, italic) instead of the style's face, for a portrait output
/// of `out` px.
#[doc(hidden)]
pub async fn test_atlas_with(
    style: &str,
    words: &[&str],
    font: Option<(&str, u16, bool)>,
    out: (u32, u32),
) -> TestAtlas {
    fonts::configure_for_tests();
    let dev = RenderDevice::new().await.expect("a GPU");
    let timed: Vec<(&str, i64, i64)> = (0i64..)
        .zip(words)
        .map(|(i, w)| (*w, i * 300_000, i * 300_000 + 250_000))
        .collect();
    let end = timed.last().map_or(1, |w| w.2);
    let pages = Pages::one_page("v", 0, end, &timed);
    let settings = CaptionSettings {
        enabled: true,
        style: Some(ContribId::builtin(style)),
        ..CaptionSettings::default()
    };
    let format = FormatKind::Portrait;
    let style = CaptionStyles::builtin()
        .for_settings(&settings, &[format])
        .0;
    let (family, weight, italic) =
        font.unwrap_or((&style.font.family, style.font.weight, style.font.italic));
    let face = family_for(&fonts::index(), &FontRef::bundled(family), weight, italic);
    let mut fonts = test_font_system();
    let mut builder = atlas::AtlasBuilder::new(&dev.device, &dev.queue);
    let width = style.pages.max_line_width.get(format);
    let mut build = || {
        let started = std::time::Instant::now();
        let laid = layout::lay_out_page(
            fonts.system(),
            &style,
            &face,
            "en",
            &pages,
            0,
            format,
            out,
            width,
        );
        let atlas = builder.build_atlas(&dev.device, &dev.queue, fonts.system(), &style, &laid);
        let _ = dev.device.poll(wgpu::PollType::Wait);
        (laid, atlas, started.elapsed().as_secs_f64() * 1e3)
    };
    let (_, _, cold_ms) = build();
    let (laid, atlas, build_ms) = build();
    TestAtlas {
        cold_ms,
        cells: laid.words.iter().map(|w| w.cell).collect(),
        size: atlas.size,
        bytes: atlas.bytes,
        build_ms,
        lines: laid.lines.len(),
        boxes: laid.words.iter().map(|w| w.size).collect(),
        em: laid.font_px * laid.autofit,
        areas: builder.glyph_areas(),
        pixels: read_back(&dev, &atlas.texture, atlas.size),
    }
}

/// `texture`'s RGBA bytes, tightly packed.
fn read_back(dev: &Arc<RenderDevice>, texture: &wgpu::Texture, (w, h): (u32, u32)) -> Vec<u8> {
    let row = (w * 4).div_ceil(256) * 256;
    let buffer = dev.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Caption atlas readback"),
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = dev.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    dev.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = dev.device.poll(wgpu::PollType::Wait);
    let mapped = slice.get_mapped_range();
    mapped
        .chunks(row as usize)
        .flat_map(|r| &r[..(w * 4) as usize])
        .copied()
        .collect()
}
