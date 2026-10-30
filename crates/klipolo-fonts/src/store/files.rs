//! The store's files: the names a family's folder and faces get, the records of finished faces (a
//! manifest's, and a download's in progress), and whether a face is still what was written.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use klipolo_model::font::font_ok;

use super::{MANIFEST, Manifest, ManifestFile, PARTIAL};

/// A file name that stays in its folder: one plain component, no separator, no `..`.
pub(crate) fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.starts_with('.')
}

/// A family's folder name: lower-case letters and digits, words joined by dashes, anything else as `_<hex>`, so a
/// family's name never makes a path (`Playfair Display` → `playfair-display`).
pub fn folder_name(family: &str) -> String {
    let mut out = String::new();
    for (i, word) in family.split_whitespace().enumerate() {
        if i > 0 {
            out.push('-');
        }
        for c in word.chars().flat_map(char::to_lowercase) {
            if c.is_ascii_alphanumeric() {
                out.push(c);
            } else {
                out.push_str(&format!("_{:x}", c as u32));
            }
        }
    }
    out
}

/// A face's file name in its folder: the family's letters and digits, its weight and `i` for italic
/// (`Testa-700i.ttf`).
pub fn face_file_name(family: &str, weight: u16, italic: bool) -> String {
    let mut stem: String = family.chars().filter(char::is_ascii_alphanumeric).collect();
    if stem.is_empty() {
        stem.push_str("Font");
    }
    format!("{stem}-{weight}{}.ttf", if italic { "i" } else { "" })
}

/// The folders in `dir` (none when it can't be read), never following a link out of it.
pub(super) fn dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

/// The finished faces recorded in `folder`: a manifest's (even one that no longer checks out) and a download's in
/// progress (`partial.json`), each with the SHA-256 of what was written.
pub(crate) fn records(folder: &Path) -> Vec<ManifestFile> {
    let read = |name: &str| fs::read(folder.join(name)).ok();
    let mut out: Vec<ManifestFile> = read(MANIFEST)
        .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
        .map(|m| m.files)
        .unwrap_or_default();
    for r in read(PARTIAL)
        .and_then(|b| serde_json::from_slice::<Vec<ManifestFile>>(&b).ok())
        .unwrap_or_default()
    {
        if !out.iter().any(|o| o.file == r.file) {
            out.push(r);
        }
    }
    out.retain(|r| plain_name(&r.file));
    out
}

/// Whether `record`'s file in `folder` is there, at its size, and hashes to what was written.
pub(crate) fn verified(folder: &Path, record: &ManifestFile) -> bool {
    let path = folder.join(&record.file);
    fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == record.bytes)
        && crate::download::check::sha256_of(&path).is_some_and(|h| h == record.sha256)
}

/// Records a download's finished faces (atomically), as each lands.
pub(crate) fn write_partial(folder: &Path, files: &[ManifestFile]) -> io::Result<()> {
    let bytes = serde_json::to_vec(files).map_err(io::Error::other)?;
    klipolo_platform::durable::write_atomic(
        &folder.join(PARTIAL),
        &bytes,
        klipolo_platform::durable::Durability::Ordered,
    )
}

/// The manifest in `folder`, when it is whole: readable, a name and version a project can hold, plain file names, and
/// every file there at its recorded size.
pub(super) fn read_manifest(folder: &Path) -> Option<Manifest> {
    let bytes = fs::read(folder.join(MANIFEST)).ok()?;
    let m: Manifest = serde_json::from_slice(&bytes).ok()?;
    let whole = font_ok(&m.font())
        && !m.files.is_empty()
        && m.files.iter().all(|f| {
            plain_name(&f.file)
                && fs::metadata(folder.join(&f.file))
                    .is_ok_and(|md| md.is_file() && md.len() == f.bytes)
        });
    whole.then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_and_file_names_never_make_a_path() {
        assert_eq!(folder_name("Playfair Display"), "playfair-display");
        assert_eq!(folder_name("../x"), "_2e_2e_2fx");
        assert_eq!(
            face_file_name("Noto Sans JP", 700, true),
            "NotoSansJP-700i.ttf"
        );
        assert_eq!(face_file_name("..", 400, false), "Font-400.ttf");
        assert!(!plain_name("../a.ttf") && !plain_name("a/b.ttf") && !plain_name(".hidden"));
    }
}
