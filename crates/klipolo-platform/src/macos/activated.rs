//! The fonts CoreText has activated: font managers (RightFont, FontBase, Typeface…) activate fonts from
//! their own folders through CoreText, where fontdb's scan of the system folders never looks. Only those files are
//! answered; the system folders' fonts are already in the scan.

use std::path::{Path, PathBuf};

use objc2_core_foundation::{CFArray, CFRetained, CFType, CFURL};
use objc2_core_text::CTFontManagerCopyAvailableFontURLs;

/// The folders fontdb's macOS scan reads itself (`Database::load_system_fonts`), and the system's own fonts (hidden
/// faces live under `/System` too).
fn scanned_already(path: &Path) -> bool {
    let home_fonts = std::env::var_os("HOME").map(|home| Path::new(&home).join("Library/Fonts"));
    path.starts_with("/System")
        || path.starts_with("/Library/Fonts")
        || path.starts_with("/Network/Library/Fonts")
        || home_fonts.is_some_and(|dir| path.starts_with(dir))
}

/// The file of every font CoreText makes available here, outside the folders the scan reads.
pub(crate) fn list() -> Vec<PathBuf> {
    // SAFETY: takes no arguments and returns a +1 retained array (the binding wraps it in `CFRetained`).
    let urls = unsafe { CTFontManagerCopyAvailableFontURLs() };
    // SAFETY: a CFArray holds CoreFoundation objects; each is checked to be a CFURL below before it is used as one.
    let urls: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(urls) };
    urls.iter()
        .filter_map(|item| item.downcast_ref::<CFURL>()?.to_file_path())
        .filter(|path| !scanned_already(path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_folders_are_left_to_the_scan() {
        assert!(scanned_already(Path::new(
            "/System/Library/Fonts/Helvetica.ttc"
        )));
        assert!(scanned_already(Path::new(
            "/Library/Fonts/Arial Unicode.ttf"
        )));
        assert!(!scanned_already(Path::new(
            "/Users/someone/Library/Application Support/FontManager/Brand Sans.otf"
        )));
    }
}
