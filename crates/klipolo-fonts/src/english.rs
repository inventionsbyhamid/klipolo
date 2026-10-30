//! klipolo-fonts' codes and their English (`fonts.*`, `klipolo_ai::english`'s pattern). `msg!` refuses to compile a code
//! that isn't here. The UI words every code in the person's language (its descriptors come from
//! `src/bindings/fontsCodes.json`); logs read `english`. Other crates add theirs.
//!
//! Parameters: `family` is a font family's name, `version` a Google version (`v31`), `status` an HTTP status.

use klipolo_model::coded::{CodedError, Messages};

#[rustfmt::skip]
pub const ENGLISH: Messages = &[
    ("fonts.missing", "{family} isn't on this computer."),
    // Google Fonts downloads: the consent and the switch, then why a download didn't happen.
    ("fonts.google.needsConsent", "Allow Google Fonts first."),
    ("fonts.google.off", "Google Fonts is off in Settings."),
    ("fonts.google.unknown", "Google Fonts has no family called {family}."),
    ("fonts.google.saveFailed", "Klipolo couldn't save this setting. Check the disk, then try again."),
    ("fonts.download.offline", "Couldn't reach Google Fonts. Check your internet connection and try again."),
    ("fonts.download.diskFull", "Your disk is full. Free up space, then try again."),
    ("fonts.download.diskError", "Klipolo couldn't save the font on this Mac. Check the disk, then try again."),
    ("fonts.download.badFile", "Google sent a file that isn't that font. Try again later."),
    ("fonts.download.tooBig", "That font file is too big."),
    ("fonts.download.cancelled", "Download cancelled."),
    ("fonts.download.http", "Google Fonts couldn't send that font right now (error {status}). Try again later."),
    ("fonts.download.first", "Download {family} first."),
    ("fonts.download.versionGone", "Google no longer offers {family} {version}. Use its current version or replace it."),
    ("fonts.remove.notOurs", "That isn't one of Klipolo's downloaded fonts."),
    ("fonts.remove.inUse", "An export that's running uses {family}. Remove it once the export is done."),
];

macro_rules! msg {
    ($($t:tt)*) => { klipolo_model::coded_msg!($crate::english::ENGLISH, $($t)*) };
}
pub(crate) use msg;

/// `e` in English, from this crate's table or the model's.
pub fn english(e: &CodedError) -> String {
    klipolo_model::coded::english(&[ENGLISH, klipolo_model::messages::MESSAGES], e)
}

/// `fonts.missing`: a text's font isn't here, so it draws in the stand-in.
pub fn missing(family: &str) -> CodedError {
    msg!("fonts.missing", family = family)
}

/// `fonts.download.versionGone`: a project's Google version that Google no longer serves.
pub fn version_gone(family: &str, version: &str) -> CodedError {
    msg!(
        "fonts.download.versionGone",
        family = family,
        version = version
    )
}

/// `fonts.google.unknown`: no family of that name in the catalogue.
pub fn unknown_google(family: &str) -> CodedError {
    msg!("fonts.google.unknown", family = family)
}

/// `fonts.google.needsConsent`, before the person's one confirmation.
pub fn needs_consent() -> CodedError {
    msg!("fonts.google.needsConsent")
}

/// `fonts.google.off`, while Settings' switch is off.
pub fn google_off() -> CodedError {
    msg!("fonts.google.off")
}

/// `fonts.google.saveFailed`: Google Fonts' setting couldn't be saved (it stays as it was).
pub fn save_failed() -> CodedError {
    msg!("fonts.google.saveFailed")
}

/// `fonts.download.first`: a change naming a Google font waits for its download (chat's and MCP's).
pub fn download_first(family: &str) -> CodedError {
    msg!("fonts.download.first", family = family)
}

/// `fonts.remove.inUse`: an export running now draws with that download.
pub fn in_use(family: &str) -> CodedError {
    msg!("fonts.remove.inUse", family = family)
}
