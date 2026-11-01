//! A take's shape images, checked as it loads. An image is usable when, links
//! followed, it is inside the take's own `cursors/` folder, is a regular file (a pipe or a device is never opened), and
//! its PNG header says at most 256 × 256. Only the 24-byte header is read: the cursor atlas decodes. An unusable shape
//! draws the Arrow look.
//!
//! The reader already refuses a shape whose file isn't exactly `cursors/<digits>.png`; this catches what the name
//! can't: a link out of the take, a missing or damaged file, an image larger than it says. Cost: at most 64 shapes,
//! each two path resolutions and one 24-byte read, on the loader thread.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_cursor::ShapeDef;
use klipolo_cursor::shapes::MAX_SHAPE_PX;

const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Each of `shapes` (in their order) from `dir`, the take's folder: its image's checked path (links followed), or None
/// when it isn't usable.
pub(super) fn usable(dir: &Path, shapes: &[ShapeDef]) -> Vec<Option<Arc<str>>> {
    let root = dir.canonicalize().ok().map(|d| d.join("cursors"));
    shapes
        .iter()
        .map(|s| {
            let real = checked(root.as_deref()?, &dir.join(&s.file))?;
            real.to_str().map(Arc::from)
        })
        .collect()
}

fn checked(root: &Path, file: &Path) -> Option<PathBuf> {
    let real = file.canonicalize().ok()?;
    let regular = std::fs::metadata(&real).is_ok_and(|m| m.is_file());
    if !real.starts_with(root) || !regular {
        return None;
    }
    // Checked again on the open file, which a pipe swapped in meanwhile can't hold up.
    let (mut f, _) = super::open_regular(&real).ok()??;
    let mut head = [0u8; 24];
    if f.read_exact(&mut head).is_err() {
        return None;
    }
    let side = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let fits = |px: u32| (1..=u32::from(MAX_SHAPE_PX)).contains(&px);
    let png = head[..8] == PNG
        && head[12..16] == *b"IHDR"
        && fits(side(&head[16..20]))
        && fits(side(&head[20..24]));
    png.then_some(real)
}
