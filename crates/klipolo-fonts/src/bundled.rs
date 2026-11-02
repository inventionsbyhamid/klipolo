//! The fonts that ship with Klipolo: Google's static TTFs of twelve open families, pinned here by size and SHA-256 (the
//! tests check every file). The rest are fallbacks only, which no menu, search, name lookup or project ever names:
//! Adobe NotDef, the last CJK fallback, and the Noto Sans of eleven scripts (`fallback::NotoScript`: Indic, Arabic,
//! Hebrew, Thai), unhinted static Regular and Bold from the notofonts releases on GitHub.

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
    /// The catalogue's version (`v20`), a Noto release's (`v2.007`), or the short commit Adobe NotDef was fetched at.
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
    face("Noto Sans Devanagari", 400, false, "NotoSansDevanagari/NotoSansDevanagari-Regular.ttf", 184228, "9c7d935139ea6a1e6ad9dbac4f6d27ece1e04bca8123c8888d00a0f9df4724cd", "v2.007", Ofl, Fallback),
    face("Noto Sans Devanagari", 700, false, "NotoSansDevanagari/NotoSansDevanagari-Bold.ttf", 183412, "ff2f76a23aad41e0608c2d7dbc4bacd247ff3bec78f0ec2a8fb106b561636e58", "v2.007", Ofl, Fallback),
    face("Noto Sans Bengali", 400, false, "NotoSansBengali/NotoSansBengali-Regular.ttf", 103704, "5dceda02816fece18ea6796f474f5d3170f1d862f21b1e809cdc94b1caa4b6ec", "v3.011", Ofl, Fallback),
    face("Noto Sans Bengali", 700, false, "NotoSansBengali/NotoSansBengali-Bold.ttf", 102440, "2fe8d779e0f19576d1fc11e39f9154b4326a17b693215017dd1f5a8898c3f2c3", "v3.011", Ofl, Fallback),
    face("Noto Sans Gurmukhi", 400, false, "NotoSansGurmukhi/NotoSansGurmukhi-Regular.ttf", 35956, "2454c944fda799d2861f11c03e492c453e33083dd1e8d99bd6baac8de71ad0f0", "v2.004", Ofl, Fallback),
    face("Noto Sans Gurmukhi", 700, false, "NotoSansGurmukhi/NotoSansGurmukhi-Bold.ttf", 35712, "d16804da302ecb2a7defe0502e65136870445fd88f9fddaef60b68d5f1ee7386", "v2.004", Ofl, Fallback),
    face("Noto Sans Gujarati", 400, false, "NotoSansGujarati/NotoSansGujarati-Regular.ttf", 142760, "18727143b8eadade0a59dca80959ef784bf8d55e07285d1b4096594dbf829f0a", "v2.106", Ofl, Fallback),
    face("Noto Sans Gujarati", 700, false, "NotoSansGujarati/NotoSansGujarati-Bold.ttf", 140812, "3ce0cf6e1d0bfe2ef5823772eeb2f25da28ec0c2c1d99c0fce529025723e61a1", "v2.106", Ofl, Fallback),
    face("Noto Sans Tamil", 400, false, "NotoSansTamil/NotoSansTamil-Regular.ttf", 46600, "6634d9cc97a726e670df41281dd32b167a6b9b71f2036e19671ff08fdde0c292", "v2.004", Ofl, Fallback),
    face("Noto Sans Tamil", 700, false, "NotoSansTamil/NotoSansTamil-Bold.ttf", 46428, "04a472e49fa83b387976756554fe179de631a950d79f652ebba14a39a44f5d71", "v2.004", Ofl, Fallback),
    face("Noto Sans Telugu", 400, false, "NotoSansTelugu/NotoSansTelugu-Regular.ttf", 165212, "dcb86a5da09365a2d73d725e429403dca988e6d50c1b1d564579a25dbc18bd29", "v2.005", Ofl, Fallback),
    face("Noto Sans Telugu", 700, false, "NotoSansTelugu/NotoSansTelugu-Bold.ttf", 165564, "44393b05a863057cc2cbfe86623fc20b906b00cf19a2ae9304cdd0f3608889db", "v2.005", Ofl, Fallback),
    face("Noto Sans Kannada", 400, false, "NotoSansKannada/NotoSansKannada-Regular.ttf", 135488, "2870274619c2a4abe5d6b11fcb7c4a372eba765b6b7881d459b0eb0a609a68eb", "v2.006", Ofl, Fallback),
    face("Noto Sans Kannada", 700, false, "NotoSansKannada/NotoSansKannada-Bold.ttf", 133868, "e989c72f6fec5a0b3366af5f90e3ce8b2b8a350fc5286c6374f0353937e513f5", "v2.006", Ofl, Fallback),
    face("Noto Sans Malayalam", 400, false, "NotoSansMalayalam/NotoSansMalayalam-Regular.ttf", 77312, "cc39bb1f7d63b582a2aec1abf2469a43b805a3523a27e0a0eee8cd32702541d5", "v2.104", Ofl, Fallback),
    face("Noto Sans Malayalam", 700, false, "NotoSansMalayalam/NotoSansMalayalam-Bold.ttf", 75916, "165d635d82b0eb0a04275b92b9334c1ece679b99f263727c24c7c92ebcab51e9", "v2.104", Ofl, Fallback),
    face("Noto Sans Arabic", 400, false, "NotoSansArabic/NotoSansArabic-Regular.ttf", 142140, "bd86ca02f087d7f3c3788ba458fb6b73744c7639ed276b8d870dba6def6c40d0", "v2.013", Ofl, Fallback),
    face("Noto Sans Arabic", 700, false, "NotoSansArabic/NotoSansArabic-Bold.ttf", 141852, "f4cb79f842a07d1bcf597879574358fb7010f73761396d1d0f2445dfedb4d2ca", "v2.013", Ofl, Fallback),
    face("Noto Sans Hebrew", 400, false, "NotoSansHebrew/NotoSansHebrew-Regular.ttf", 16836, "04272f5600d0ec816d31d0df73b23aa8d3501ea359ebe820da31c11ffcf00853", "v3.001", Ofl, Fallback),
    face("Noto Sans Hebrew", 700, false, "NotoSansHebrew/NotoSansHebrew-Bold.ttf", 16900, "dfdb3056de1f4542b888c77a1a8a750548a802e271479f56e52152423b64dde8", "v3.001", Ofl, Fallback),
    face("Noto Sans Thai", 400, false, "NotoSansThai/NotoSansThai-Regular.ttf", 20960, "d4303fe9c63ebb72759ca8b6d2040c8ae81689f7d08d7b91c656154382b49313", "v2.002", Ofl, Fallback),
    face("Noto Sans Thai", 700, false, "NotoSansThai/NotoSansThai-Bold.ttf", 20600, "5227602d7f9108252cfb7d75d6f7b2a26bdb2fc2fa89a702f9063758fb2f86c7", "v2.002", Ofl, Fallback),
];

/// The bundled families, each once, in `BUNDLED`'s order (the twelve people pick, then the fallbacks).
pub fn bundled_families() -> impl Iterator<Item = &'static str> {
    BUNDLED
        .iter()
        .enumerate()
        .filter(|(i, f)| *i == 0 || BUNDLED[i - 1].family != f.family)
        .map(|(_, f)| f.family)
}

/// Whether `family` is a bundled fallback only (Adobe NotDef, a script's Noto Sans): never a family a text uses.
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
