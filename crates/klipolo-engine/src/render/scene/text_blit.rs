//! Glyph images drawn into a text atlas's pixels (`text_atlas.rs`): a mask's coverage over the fill and
//! the silhouette, an outline's over the silhouette, a colour glyph premultiplied over the colour plane (only as large as
//! the colour glyphs); a glyph's outline stroked, and a piece's glyphs and ink box.

use std::collections::HashMap;

use crate::render::effects::made;
use glyphon::FontSystem;
use glyphon::cosmic_text::{CacheKey, LayoutGlyph, SwashContent, SwashImage};
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Join, Stroke, Vector};

use super::text::{Piece, TextUnits};
use super::text_atlas::AtlasImage;

/// One glyph ready to draw: its image's box at the scale (whole pixels, at its place in the layout) and its key, and
/// its outline's box and image, if it has one.
pub(super) struct Ready {
    pub key: CacheKey,
    pub x: i32,
    pub y: i32,
    pub stroke: Option<(i32, i32, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Channel {
    Fill,
    Silhouette,
}

/// `img` drawn at (x, y) into `out`: a mask's coverage over R (the fill) and G (the silhouette), or only G (an
/// outline); a colour glyph premultiplied over the colour plane, its alpha into the silhouette.
pub(super) fn blit(out: &mut AtlasImage, img: &SwashImage, x: i32, y: i32, channel: Channel) {
    let (w, h) = (img.placement.width as i32, img.placement.height as i32);
    let (aw, ah) = (out.size.0 as i32, out.size.1 as i32);
    let over = |dst: &mut u8, a: u8| {
        let d = u32::from(*dst);
        *dst = (d + u32::from(a) * (255 - d) / 255) as u8;
    };
    for row in 0..h {
        let ty = y + row;
        if ty < 0 || ty >= ah {
            continue;
        }
        for col in 0..w {
            let tx = x + col;
            if tx < 0 || tx >= aw {
                continue;
            }
            let at = (ty * aw + tx) as usize;
            let src = (row * w + col) as usize;
            match img.content {
                SwashContent::Color => {
                    let Some(c) = img.data.get(src * 4..src * 4 + 4) else {
                        continue;
                    };
                    let a = u32::from(c[3]);
                    over(&mut out.masks[at * 2 + 1], c[3]);
                    let Some(plane) = &mut out.colors else {
                        continue;
                    };
                    let (px, py) = (tx - plane.at[0] as i32, ty - plane.at[1] as i32);
                    let (pw, ph) = (plane.size.0 as i32, plane.size.1 as i32);
                    if px < 0 || py < 0 || px >= pw || py >= ph {
                        continue;
                    }
                    let p = (py * pw + px) as usize * 4;
                    let d = &mut plane.pixels[p..p + 4];
                    for k in 0..3 {
                        let s = u32::from(c[k]) * a / 255;
                        d[k] = (s + u32::from(d[k]) * (255 - a) / 255).min(255) as u8;
                    }
                    over(&mut d[3], c[3]);
                }
                _ => {
                    let a = img.data.get(src).copied().unwrap_or(0);
                    if channel == Channel::Fill {
                        over(&mut out.masks[at * 2], a);
                    }
                    over(&mut out.masks[at * 2 + 1], a);
                }
            }
        }
    }
}

/// Piece `p`'s glyphs in `t`.
pub(super) fn glyphs_of<'a>(t: &'a TextUnits, p: &Piece) -> &'a [LayoutGlyph] {
    t.glyphs
        .get(p.glyphs[0] as usize..p.glyphs[1] as usize)
        .unwrap_or_default()
}

/// `ink` grown to hold `img` drawn with its top left at (x, y).
pub(super) fn grow(ink: &mut [i32; 4], x: i32, y: i32, img: &SwashImage) {
    if img.placement.width == 0 || img.placement.height == 0 {
        return;
    }
    *ink = [
        ink[0].min(x),
        ink[1].min(y),
        ink[2].max(x + img.placement.width as i32),
        ink[3].max(y + img.placement.height as i32),
    ];
}

/// What outlines are stroked with and kept in: the scaler's context, the images, and each one's index by glyph and
/// width.
pub(super) type Strokes<'a> = (
    &'a mut ScaleContext,
    &'a mut Vec<SwashImage>,
    &'a mut HashMap<(CacheKey, u32), usize>,
);

/// The glyph `key`'s outline, `width` pixels out from its edge, round-joined (cached per glyph and width).
pub(super) fn stroke_of(
    fs: &mut FontSystem,
    (context, strokes, stroked): &mut Strokes,
    key: CacheKey,
    width: f32,
) -> Option<usize> {
    if let Some(&k) = stroked.get(&(key, width.to_bits())) {
        return Some(k);
    }
    let font = fs.get_font(key.font_id)?;
    let mut scaler = context
        .builder(font.as_swash())
        .size(f32::from_bits(key.font_size_bits))
        .hint(true)
        .build();
    let mut stroke = Stroke::new(2.0 * width);
    stroke.join(Join::Round);
    let image = Render::new(&[Source::Outline])
        .style(stroke)
        .offset(Vector::new(key.x_bin.as_float(), key.y_bin.as_float()))
        .render(&mut scaler, key.glyph_id)?;
    strokes.push(image);
    stroked.insert((key, width.to_bits()), strokes.len() - 1);
    Some(strokes.len() - 1)
}

/// An atlas texture of `size` in `format` holding `pixels`, uploaded once (on the text build thread).
pub(super) fn atlas_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    pixels: &[u8],
) -> wgpu::Texture {
    let extent = wgpu::Extent3d {
        width: size.0.max(1),
        height: size.1.max(1),
        depth_or_array_layers: 1,
    };
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Klipolo scene text atlas"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let per_px = (pixels.len() / (extent.width as usize * extent.height as usize).max(1)) as u32;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &t,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(extent.width * per_px),
            rows_per_image: Some(extent.height),
        },
        extent,
    );
    made();
    t
}
