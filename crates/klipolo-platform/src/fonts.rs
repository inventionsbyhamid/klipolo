//! Fonts the OS decides: the colour emoji face at the end of the fallback chain, where
//! The bundled fonts sit beside the app, and the font files a font manager activated. Shared code names no
//! OS family. There is deliberately no list of system CJK families: system CJK faces are never fallbacks, only drawn
//! when the person picks one.

use std::path::{Path, PathBuf};

use crate::caps::OsFamily;

/// This OS's colour emoji family (`emoji_family_of` this build's OS).
pub fn emoji_family() -> &'static str {
    emoji_family_of(crate::caps::FAMILY)
}

/// Each OS's colour emoji family: Apple Color Emoji, Segoe UI Emoji, Noto Color Emoji. One table, so a file every build
/// shares (the webview's `font-fallbacks.json`, generated on any machine) carries each OS's face and each build takes
/// its own.
pub fn emoji_family_of(os: OsFamily) -> &'static str {
    match os {
        OsFamily::Macos => "Apple Color Emoji",
        OsFamily::Windows => "Segoe UI Emoji",
        OsFamily::Linux => "Noto Color Emoji",
    }
}

/// The bundled fonts folder of an app whose executable is in `exe_dir`: `../Resources/fonts` in a macOS app bundle,
/// `fonts\` beside the exe on Windows, `fonts/` beside the binary on Linux.
pub fn bundled_fonts_dir(exe_dir: &Path) -> PathBuf {
    crate::imp::fonts::bundled_fonts_dir(exe_dir)
}

/// Font files this computer has activated outside the folders every font scan reads: on macOS, fonts a font manager
/// activates through CoreText (`CTFontManagerCopyAvailableFontURLs`), which live in the manager's own folders. The
/// engine's one scan loads them beside the system's. Windows and Linux give none: their fonts are in the folders the
/// scan reads (tests can set the list: `fake::fonts::set_activated`).
pub fn activated_font_files() -> Vec<PathBuf> {
    #[cfg(any(test, feature = "fake"))]
    if let Some(files) = crate::fake::fonts::activated_override() {
        return files;
    }
    crate::imp::fonts::activated_font_files()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_folder_and_the_emoji_face_are_this_os_own() {
        let exe = Path::new("/app/bin");
        #[cfg(target_os = "macos")]
        let (folder, emoji) = (exe.join("../Resources/fonts"), "Apple Color Emoji");
        #[cfg(windows)]
        let (folder, emoji) = (exe.join("fonts"), "Segoe UI Emoji");
        #[cfg(target_os = "linux")]
        let (folder, emoji) = (exe.join("fonts"), "Noto Color Emoji");
        assert_eq!(bundled_fonts_dir(exe), folder);
        assert_eq!(emoji_family(), emoji);
        assert_eq!(
            crate::fake::fonts::bundled_fonts_dir(exe),
            exe.join("fonts")
        );
        assert_eq!(crate::fake::fonts::emoji_family(), "Noto Color Emoji");
    }

    #[test]
    fn activated_fonts_can_be_set_for_tests() {
        let files = vec![PathBuf::from("/Fonts/Managed/Brand Sans.otf")];
        crate::fake::fonts::set_activated(Some(files.clone()));
        assert_eq!(activated_font_files(), files);
        crate::fake::fonts::set_activated(None);
        #[cfg(not(target_os = "macos"))]
        assert!(activated_font_files().is_empty());
    }
}
