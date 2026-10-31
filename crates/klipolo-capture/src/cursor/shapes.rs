//! The shape store: each image the source saw is stored once, as `cursors/<id>.png`, and defined by an
//! `S` line carrying the source's key, so a resumed take continues the ids. At most
//! `klipolo_cursor::MAX_SHAPES`; past that a new shape draws as the Arrow look. The PNG is written whole before its line
//! (`Effect::WriteAtomic`): a crash leaves it complete or absent.
//!
//! Memory: at most 64 keys. A PNG is encoded once per new shape, never per frame.

use std::collections::HashMap;

use klipolo_cursor::format::Line;
use klipolo_cursor::shapes::MAX_SHAPE_PX;
use klipolo_cursor::{CursorTrack, MAX_SHAPES};

use super::ShapeImage;
use crate::session::{CURSORS_DIR, FileKey};

/// Shape ids by the source's key (see the module notes).
#[derive(Clone, Debug, Default)]
pub struct ShapeStore {
    by_key: HashMap<u64, u16>,
    next: u16,
}

impl ShapeStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The store as a resumed take left it: the keys and the next id from its `S` lines.
    pub fn resumed(track: &CursorTrack) -> Self {
        let defs = track.shapes();
        Self {
            by_key: defs.iter().map(|d| (d.key, d.id)).collect(),
            next: defs
                .iter()
                .map(|d| d.id.saturating_add(1))
                .max()
                .unwrap_or(0),
        }
    }

    pub fn id_of(&self, key: u64) -> Option<u16> {
        self.by_key.get(&key).copied()
    }

    /// No room for another shape.
    pub fn is_full(&self) -> bool {
        usize::from(self.next) >= MAX_SHAPES
    }

    /// A new shape's id, its PNG file and its `S` line; None past `MAX_SHAPES` or for an image the reader would refuse
    /// (the shape then draws as the Arrow look; the writer gives one `capture.note.cursorShapes` note when full).
    pub fn add(&mut self, key: u64, image: &ShapeImage) -> Option<(u16, FileKey, Vec<u8>, Line)> {
        if self.is_full() || !drawable(image) {
            return None;
        }
        let png = encode_png(image);
        if png.is_empty() {
            return None;
        }
        let id = self.next;
        self.next += 1;
        self.by_key.insert(key, id);
        let file = format!("{CURSORS_DIR}/{id}.png");
        let line = Line::ShapeDef {
            id,
            key: format!("{key:016x}"),
            file: file.clone(),
            pw: image.px.0,
            ph: image.px.1,
            hx: image.hotspot_px.0,
            hy: image.hotspot_px.1,
            pts: image.pts,
            sys: image.sys,
        };
        Some((id, file, png, line))
    }
}

/// An image the file can hold: 1-256 px a side, its bytes RGBA8 for that size, a hotspot inside it (its edges count)
/// and a positive height in points. Anything else would be refused by the reader.
fn drawable(image: &ShapeImage) -> bool {
    let (w, h) = image.px;
    let side = |n: u16| (1..=MAX_SHAPE_PX).contains(&n);
    let (hx, hy) = image.hotspot_px;
    side(w)
        && side(h)
        && image.rgba.len() == usize::from(w) * usize::from(h) * 4
        && (0.0..=f32::from(w)).contains(&hx)
        && (0.0..=f32::from(h)).contains(&hy)
        && image.pts.is_finite()
        && image.pts > 0.0
}

/// The image as a PNG: RGBA8, fast compression. Empty if it couldn't be encoded (a size that doesn't match its bytes).
pub fn encode_png(image: &ShapeImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.rgba.len() / 2 + 128);
    let mut enc = png::Encoder::new(&mut out, u32::from(image.px.0), u32::from(image.px.1));
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let written = enc
        .write_header()
        .and_then(|mut w| w.write_image_data(&image.rgba).and_then(|()| w.finish()));
    match written {
        Ok(()) => out,
        Err(e) => {
            tracing::warn!(error = %e, "cursor: a shape's image couldn't be encoded");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u16, h: u16) -> ShapeImage {
        ShapeImage {
            px: (w, h),
            rgba: vec![200; usize::from(w) * usize::from(h) * 4],
            hotspot_px: (1.0, 1.0),
            pts: 16.0,
            sys: None,
        }
    }

    #[test]
    fn each_shape_is_stored_once_up_to_the_limit_and_decodes() {
        let mut store = ShapeStore::new();
        let (id, file, png, line) = store.add(0xabc, &image(32, 32)).unwrap();
        assert_eq!((id, file.as_str()), (0, "cursors/0.png"));
        assert!(matches!(line, Line::ShapeDef { ref key, .. } if key == "0000000000000abc"));
        let decoded = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        assert_eq!((decoded.info().width, decoded.info().height), (32, 32));
        assert_eq!(store.id_of(0xabc), Some(0));
        let mut bad = image(4, 4);
        bad.rgba.pop();
        assert!(store.add(1, &bad).is_none(), "bytes that aren't its size");
        bad = image(4, 4);
        bad.hotspot_px = (5.0, 1.0);
        assert!(store.add(1, &bad).is_none(), "a hotspot outside the image");
        for key in 1..MAX_SHAPES as u64 {
            assert!(store.add(key, &image(2, 2)).is_some());
        }
        assert!(store.is_full() && store.add(999, &image(2, 2)).is_none());
    }
}
