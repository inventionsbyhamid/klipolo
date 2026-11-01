//! The fonts that ship with Klipolo: Google's static TTFs of twelve open families, fetched once by
//! `scripts/fetch-bundled-fonts.mjs` and pinned here by size and SHA-256 (the tests check every file, and
//! `scripts/check-release.mjs` reads these rows to check the app's copies). A thirteenth, Adobe NotDef, is the last CJK
//! fallback only: no menu, search, name lookup or project ever names it.

use std::path::PathBuf;

/// A licence file, beside the family's files in its folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Licence {
    /// SIL Open Font License 1.1, as Google Fonts ships it (`OFL.txt`).
    Ofl,
    /// Apache License 2.0 (`LICENSE.txt`).
    Apache,
    /// SIL Open Font License 1.1, as Adobe's repository ships it (`LICENSE.md`).
    OflMd,
}

impl Licence {
    pub fn file_name(self) -> &'static str {
        match self {
            Licence::Ofl => "OFL.txt",
            Licence::Apache => "LICENSE.txt",
            Licence::OflMd => "LICENSE.md",
        }
    }

    /// Its SPDX identifier, as `fonts.txt` names it.
    pub fn spdx(self) -> &'static str {
        match self {
            Licence::Ofl | Licence::OflMd => "OFL-1.1",
            Licence::Apache => "Apache-2.0",
        }
    }
}

/// The font menu's categories, plus `Fallback`, which no menu, search or name lookup ever lists.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Category {
    Sans,
    Serif,
    Display,
    Handwriting,
    Mono,
    Fallback,
}

/// One bundled file: one face of one family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BundledFace {
    /// The catalogue's name, which the face is registered under (never the file's own: Inter's SemiBold calls itself
    /// "Inter SemiBold").
    pub family: &'static str,
    pub weight: u16,
    pub italic: bool,
    /// Under the bundled folder (`assets/fonts` in the repository, `Contents/Resources/fonts` in the app).
    pub file: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    /// The catalogue's version (`v20`), or the short commit Adobe NotDef was fetched at.
    pub version: &'static str,
    pub licence: Licence,
    pub category: Category,
}

#[allow(clippy::too_many_arguments)]
const fn face(
    family: &'static str,
    weight: u16,
    italic: bool,
    file: &'static str,
    bytes: u64,
    sha256: &'static str,
    version: &'static str,
    licence: Licence,
    category: Category,
) -> BundledFace {
    BundledFace {
        family,
        weight,
        italic,
        file,
        bytes,
        sha256,
        version,
        licence,
        category,
    }
}

use Category::*;
use Licence::*;

/// Every bundled face, a family's faces together. One row a line (`scripts/check-release.mjs` reads them).
#[rustfmt::skip]
pub const BUNDLED: &[BundledFace] = &[
    face("Inter", 400, false, "Inter/Inter-Regular.ttf", 342408, "a414b48aa577ef2c62ebb135341ddeef33ee26a4f5dc9f787f93c1aab08ebb50", "v20", Ofl, Sans),
    face("Inter", 600, false, "Inter/Inter-SemiBold.ttf", 343632, "f30e9d2574c3bec5144347ff965f9841c8f06857f0b7383000f8c9489a161841", "v20", Ofl, Sans),
    face("Inter", 700, false, "Inter/Inter-Bold.ttf", 344072, "fbe825cbfc749317f57a1433d3905aa9d3f388732fd29b8e8a36f34845e3bc3e", "v20", Ofl, Sans),
    face("TikTok Sans", 600, false, "TikTokSans/TikTokSans-SemiBold.ttf", 202724, "0305cc96b4a454dc0d76fb6adf21d29927f1b75804b69bc7f245ffb0b45bfeb3", "v7", Ofl, Sans),
    face("TikTok Sans", 800, false, "TikTokSans/TikTokSans-ExtraBold.ttf", 202760, "1e636f18c7d55e91a145ac2b571ee1d304f344ef9c514e7fa244b28f35c0cbbb", "v7", Ofl, Sans),
    face("Montserrat", 900, false, "Montserrat/Montserrat-Black.ttf", 343296, "b48e6741b9889ede4ffe3d8acfda3e6917c62127537b19fe7206cba04e6ebb63", "v31", Ofl, Sans),
    face("Anton", 400, false, "Anton/Anton-Regular.ttf", 161588, "97aae409210d255fb3b92f18c8af2ed14941e10a1ee134a35bb91f52086de1d0", "v27", Ofl, Sans),
    face("Bangers", 400, false, "Bangers/Bangers-Regular.ttf", 92468, "1f9bcc5a779979918a0b8bbff38c87378040f0e52a45eecd47be7b2c6da363c2", "v25", Ofl, Display),
    face("Oswald", 700, false, "Oswald/Oswald-Bold.ttf", 87600, "21843a0ccca2e97030da1330961646510ed6e9b8bb39620974c1195b9803034a", "v57", Ofl, Sans),
    face("Rubik", 900, true, "Rubik/Rubik-BlackItalic.ttf", 206960, "02e1af00373478d72511a526a83285167ae8b5c60edfdbf59a5d8c28f2f9c5a7", "v31", Ofl, Sans),
    face("Bebas Neue", 400, false, "BebasNeue/BebasNeue-Regular.ttf", 57676, "830ea186acffc2316ed1a4e42319246ba3b46b04e33a211079249bf901193f04", "v16", Ofl, Display),
    face("Luckiest Guy", 400, false, "LuckiestGuy/LuckiestGuy-Regular.ttf", 58324, "fbe683091b0db0faa8f38e5227ad3bc17dc67f119f27ff10a63d72f5f9bb9da3", "v25", Apache, Display),
    face("Playfair Display", 400, false, "PlayfairDisplay/PlayfairDisplay-Regular.ttf", 193380, "861f838d481d28cbbd4793e45dc02f01d04c81e06ed98ab2779ca152ace9f27b", "v40", Ofl, Serif),
    face("Playfair Display", 700, false, "PlayfairDisplay/PlayfairDisplay-Bold.ttf", 193720, "4f3b87b5aa297eed5e5a48dbd9941356ca0313d4725b02c29a298cb042b7b31b", "v40", Ofl, Serif),
    face("Caveat", 700, false, "Caveat/Caveat-Bold.ttf", 257316, "b42cc772f3528d9150c29d6051e2ed3fd472ad4e584c73fe1500ee628d6b1ac0", "v23", Ofl, Handwriting),
    face("JetBrains Mono", 400, false, "JetBrainsMono/JetBrainsMono-Regular.ttf", 114904, "b6b1ff4ddefe36d7f2a6174e1d001cab374e594519ee9049af028d577b64c5f5", "v24", Ofl, Mono),
    face("JetBrains Mono", 700, false, "JetBrainsMono/JetBrainsMono-Bold.ttf", 114828, "a2349098b9e45419e7bf0e2958d6c4937a049dded37387b08be725be4c7615f3", "v24", Ofl, Mono),
    face("Adobe NotDef", 400, false, "AdobeNotDef/AND-Regular.ttf", 219320, "dd734d4e2a40b380a1b5ac4adbb5985929ca6b0e2808cce62609343870bef543", "1f1f863", OflMd, Fallback),
];

/// The bundled families, each once, in `BUNDLED`'s order (the twelve people pick, then Adobe NotDef).
pub fn bundled_families() -> impl Iterator<Item = &'static str> {
    BUNDLED
        .iter()
        .enumerate()
        .filter(|(i, f)| *i == 0 || BUNDLED[i - 1].family != f.family)
        .map(|(_, f)| f.family)
}

/// Whether `family` is a bundled fallback only (Adobe NotDef): never a family a text uses.
pub fn fallback_only(family: &str) -> bool {
    BUNDLED
        .iter()
        .any(|f| f.family == family && f.category == Category::Fallback)
}

/// Where the bundled files are: `KLIPOLO_FONTS_DIR`; else the folder beside this executable
/// (`klipolo_platform::fonts::bundled_fonts_dir`); else, in debug builds only, the repository's `assets/fonts`. None for
/// a broken install, which draws every text in the system sans-serif and says so (`frame.fontMissing`).
pub fn bundled_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KLIPOLO_FONTS_DIR") {
        return Some(PathBuf::from(dir));
    }
    let exe = std::env::current_exe().ok()?;
    let beside = klipolo_platform::fonts::bundled_fonts_dir(exe.parent()?);
    if beside.join("Inter").is_dir() {
        return Some(beside);
    }
    #[cfg(debug_assertions)]
    {
        let repo = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts"));
        if repo.is_dir() {
            return Some(repo);
        }
    }
    None
}
