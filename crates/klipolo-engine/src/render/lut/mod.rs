//! `.cube` 3D LUTs (Adobe/Resolve format): red changes fastest, then green, then blue. `LutTextures` (`gpu.rs`)
//! uploads them as `Rgba16Float` 3D textures for the composite shader's `@group(1)` (sampled after the colour grade).

mod gpu;
mod upload;

use std::path::Path;

pub use gpu::LutTextures;

#[derive(Debug)]
pub struct CubeLut {
    pub size: u32,
    pub data: Vec<[f32; 3]>,
}

pub fn parse_cube(text: &str) -> Result<CubeLut, String> {
    const DAMAGED: &str = "This LUT file is damaged.";
    let (mut size, mut min, mut max) = (None, [0.0f32; 3], [1.0f32; 3]);
    let mut data = Vec::new();
    // Three numbers from the words of a line (no per-line allocation).
    fn triple<'a>(mut words: impl Iterator<Item = &'a str>) -> Option<[f32; 3]> {
        Some([
            words.next()?.parse().ok()?,
            words.next()?.parse().ok()?,
            words.next()?.parse().ok()?,
        ])
    }
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(first) = words.next() else { continue };
        if first.starts_with('#') {
            continue;
        }
        match first {
            "LUT_3D_SIZE" => {
                size = words.next().and_then(|n| n.parse::<u32>().ok());
                if let Some(n) = size.filter(|n| (2..=129).contains(n)) {
                    data.reserve_exact((n * n * n) as usize);
                }
            }
            "LUT_1D_SIZE" => return Err("Only 3D LUTs (.cube) are supported.".into()),
            "DOMAIN_MIN" => min = triple(words).ok_or(DAMAGED)?,
            "DOMAIN_MAX" => max = triple(words).ok_or(DAMAGED)?,
            "TITLE" | "LUT_3D_INPUT_RANGE" => {}
            _ => data.push(triple(std::iter::once(first).chain(words)).ok_or(DAMAGED)?),
        }
    }
    let size = size.ok_or("This LUT file has no LUT_3D_SIZE line.")?;
    if !(2..=129).contains(&size) || data.len() != (size * size * size) as usize {
        return Err("This LUT file is incomplete.".into());
    }
    for v in &mut data {
        for c in 0..3 {
            v[c] = (v[c] - min[c]) / (max[c] - min[c]).max(f32::EPSILON);
        }
    }
    Ok(CubeLut { size, data })
}

/// What the preview says when a clip's LUT can't be used (the clip draws without it).
pub fn lut_warning(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);
    format!("This LUT couldn't be loaded: {name}")
}
