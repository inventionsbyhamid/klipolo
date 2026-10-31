//! The app's own codes for editing copies (`copies.*`) and their English: Rust never sends English for the page to
//! show. `code!` refuses to compile a code `APP` doesn't have. The page words them from
//! `apps/desktop/src/bindings/appCodes.json`, which `export_bindings_app_codes` writes (`pnpm bindings`); logs and
//! tests read them with `klipolo_model::coded::english`.
//!
//! Parameters: `name` is a file's name.

use klipolo_model::coded::Messages;

/// The code itself (`&'static str`), checked against `APP` when the app compiles.
macro_rules! code {
    ($c:literal) => {
        klipolo_model::coded_code!($crate::english::APP, $c)
    };
}
pub(crate) use code;

#[rustfmt::skip]
pub const APP: Messages = &[
    // Why an editing copy is paused: it goes on by itself, or at Resume all.
    ("copies.paused.playing", "Paused while the video plays. It goes on when you stop."),
    ("copies.paused.exporting", "Paused while you export. It goes on when the export ends."),
    ("copies.paused.recording", "Paused while you record. It goes on when the take ends."),
    ("copies.paused.analyzing", "Paused while Klipolo analyzes your video. It goes on by itself."),
    ("copies.paused.held", "Paused. Press Resume all to go on."),
    ("copies.paused.diskFull", "Paused because the disk is almost full. Free up space, then press Resume all."),
    ("copies.paused.hot", "Paused because this Mac is running hot. It tries again in 5 minutes."),
    ("copies.paused.encoderBusy", "Paused because the video encoder is busy. It tries again by itself."),
    ("copies.paused.workerStopped", "Making the copy stopped unexpectedly. Trying again."),
    // Why a copy wasn't made: it waits for Try again. A missing file's words are the preview's notice's too.
    ("copies.failed.missing", "{name} isn't where it was. Put it back, then open the project again."),
    ("copies.failed.unreadable", "{name} can't be read as video, so it has no editing copy. The preview plays the file itself."),
    ("copies.failed.unknownLength", "{name} doesn't say how long it is, so it has no editing copy. The preview plays the file itself."),
    ("copies.failed.unusualRate", "{name} has no usable frame rate, so it has no editing copy. The preview plays the file itself."),
    ("copies.failed.cacheUnwritable", "Klipolo can't save editing copies on this disk. Check that it isn't full or read-only, then press Try again."),
    ("copies.failed.encoderBusy", "The video encoder stayed busy, so the editing copy of {name} wasn't made. Press Try again later."),
    ("copies.failed.worker", "Klipolo couldn't start making the editing copy of {name}. Press Try again."),
    ("copies.failed.workerKeepsStopping", "Making the copy keeps stopping. Press Try again later."),
    ("copies.failed.other", "Klipolo couldn't make an editing copy of {name}. Press Try again."),
    // Why a command about copies wasn't taken.
    ("copies.refused.notInProject", "That file isn't in the open project, so its editing copy didn't change."),
    ("copies.refused.unavailable", "Editing copies aren't available right now. Try again in a moment."),
    ("copies.refused.slowDrive", "A slow disk is holding up editing copies. Try again in a moment."),
];

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::APP;

    /// `pnpm bindings` runs this (its name matches `export_bindings`): `apps/desktop/src/bindings/appCodes.json`, the
    /// app's codes, their English and their values, which the page words them from (as `fontsCodes.json`).
    #[test]
    fn export_bindings_app_codes() {
        let list: Vec<serde_json::Value> = APP
            .iter()
            .map(|(code, message)| {
                let params: Vec<&str> = message
                    .split('{')
                    .skip(1)
                    .filter_map(|s| s.split('}').next())
                    .collect();
                serde_json::json!({ "code": code, "message": message, "params": params })
            })
            .collect();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/bindings/appCodes.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_string_pretty(&list).unwrap() + "\n").unwrap();
    }

    /// Each code once, in the app's namespace, in plain sentences: no em or en dashes, and only `name` as a value.
    #[test]
    fn every_code_is_a_plain_sentence() {
        let mut seen = std::collections::HashSet::new();
        for (code, message) in APP {
            assert!(seen.insert(code), "{code} twice");
            assert!(code.starts_with("copies."), "{code}");
            assert!(
                !message.contains(['\u{2014}', '\u{2013}']),
                "{code}: {message}"
            );
            assert!(message.ends_with('.'), "{code}: {message}");
            let values = message
                .split('{')
                .skip(1)
                .filter_map(|s| s.split('}').next());
            for value in values {
                assert_eq!(value, "name", "{code}");
            }
        }
    }
}
