//! macOS: the app bundle's fonts folder (`Contents/Resources/fonts`), and the fonts CoreText has activated (`activated`).

use std::path::{Path, PathBuf};

/// `Contents/MacOS/klipolo` → `Contents/Resources/fonts`.
pub(crate) fn bundled_fonts_dir(exe_dir: &Path) -> PathBuf {
    exe_dir.join("../Resources/fonts")
}

/// The font files CoreText lists outside the folders fontdb's scan reads (`activated::list`).
pub(crate) fn activated_font_files() -> Vec<PathBuf> {
    super::activated::list()
}
