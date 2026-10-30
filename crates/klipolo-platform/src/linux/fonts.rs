//! Linux: the fonts folder beside the binary.

use std::path::{Path, PathBuf};

pub(crate) fn bundled_fonts_dir(exe_dir: &Path) -> PathBuf {
    exe_dir.join("fonts")
}

/// None: the fonts a font manager installs are in the folders fontdb's scan reads.
pub(crate) fn activated_font_files() -> Vec<PathBuf> {
    Vec::new()
}
