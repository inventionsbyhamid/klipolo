//! What a downloaded file must be before it is used: a static face of the weight and style the catalogue
//! names, read with ttf-parser (memory-safe) from a read-only map.

use std::fs::File;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::FontError;
use crate::coverage::{Coverage, coverage_of};

/// Whether a face whose OS/2 weight is `file` is the catalogue's `named` weight. Google's static files keep Thin at 250
/// and ExtraLight at 275 (fontbakery's usWeightClass rule, for Windows GDI): those are 100 and 200.
pub fn weight_ok(file: u16, named: u16) -> bool {
    file == named || (named == 100 && file == 250) || (named == 200 && file == 275)
}

/// What a checked face gives the manifest (its SHA-256 was taken as it was written: `net::fetch`).
pub(super) struct Checked {
    pub coverage: Coverage,
    /// Its copyright notice (name ID 0), for the licence file.
    pub copyright: Option<String>,
}

/// `path` is a static font whose face is `weight` and `italic`: its coverage and its copyright line. Else `BadFile`.
pub(super) fn check_face(path: &Path, weight: u16, italic: bool) -> Result<Checked, FontError> {
    let file = File::open(path).map_err(|e| FontError::disk(&e))?;
    // SAFETY: read-only, dropped before this returns, and only ttf-parser (bounds-checked) reads it. A file changed
    // underneath gives wrong bytes, which the check refuses.
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| FontError::disk(&e))?;
    let face =
        ttf_parser::Face::parse(&map, 0).map_err(|_| FontError::BadFile("not a font".into()))?;
    if face.is_variable() {
        return Err(FontError::BadFile("a variable font".into()));
    }
    let got = face.weight().to_number();
    if !weight_ok(got, weight) || face.is_italic() != italic {
        return Err(FontError::BadFile(format!(
            "a {got} {} face, not {weight} {}",
            if face.is_italic() {
                "italic"
            } else {
                "upright"
            },
            if italic { "italic" } else { "upright" },
        )));
    }
    let copyright = face
        .names()
        .into_iter()
        .filter(|n| n.name_id == ttf_parser::name_id::COPYRIGHT_NOTICE && n.is_unicode())
        .find_map(|n| n.to_string())
        .map(|s| s.chars().filter(|c| !c.is_control()).take(500).collect());
    Ok(Checked {
        coverage: coverage_of(&face),
        copyright,
    })
}

/// The SHA-256 (hex) of the file at `path`, read from a map; None when it can't be read.
pub(crate) fn sha256_of(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    // SAFETY: read-only, dropped before this returns; only the hasher reads it.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some(hex(&Sha256::digest(&map[..])))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn googles_thin_and_extralight_weights_are_theirs() {
        assert!(
            super::weight_ok(250, 100) && super::weight_ok(275, 200) && super::weight_ok(400, 400)
        );
        assert!(!super::weight_ok(400, 700) && !super::weight_ok(250, 300));
    }
}
