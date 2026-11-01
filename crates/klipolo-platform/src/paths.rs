//! Known folders and file names that work on every OS.

use std::ffi::OsString;
use std::path::PathBuf;

/// Names Windows reserves for devices, with or without an extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A name (a project's, for an export) that is safe as a file name on every OS, so a name made on a Mac also works
/// on Windows: `/ : < > " \ | ? *` become dashes; control characters go; no leading or trailing dots or spaces; at
/// most 100 characters; a Windows device name (`CON`, `nul.txt`, `COM1`…) gets an underscore after it; an empty
/// result becomes "Untitled video".
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | ':' | '<' | '>' | '"' | '\\' | '|' | '?' | '*' => '-',
            c => c,
        })
        .filter(|c| !c.is_control())
        .collect();
    let capped: String = cleaned
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .chars()
        .take(100)
        .collect();
    let capped = capped.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if capped.is_empty() {
        return "Untitled video".into();
    }
    let stem_len = capped.find('.').unwrap_or(capped.len());
    let stem = capped[..stem_len].trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return format!("{stem}_{}", &capped[stem_len..]);
    }
    capped.into()
}

/// The user's videos folder: `~/Movies` on macOS, the Videos known folder (`FOLDERID_Videos`) on Windows, and
/// `XDG_VIDEOS_DIR` on Linux, else `~/Videos` (minimal desktops have no `user-dirs.dirs`). None: no home folder.
pub fn videos_dir() -> Option<PathBuf> {
    crate::imp::videos_dir()
}

/// Where recordings are made: `<videos>/Klipolo/Recordings` on every OS, one `<id>.klipolorec` folder per take. Never a
/// cache or temporary folder, which the OS may empty. `KLIPOLO_RECORDINGS_ROOT` replaces it when it is an absolute path
/// (to record onto a small disk image for the disk-full check); a relative one is ignored, with a warning in the log,
/// since the app and its recorder would read it from different folders. None: no videos folder.
pub fn recordings_root() -> Option<PathBuf> {
    recordings_root_in(std::env::var_os("KLIPOLO_RECORDINGS_ROOT"), videos_dir())
}

/// [`recordings_root`] with the variable's value `over` and the videos folder `videos`.
fn recordings_root_in(over: Option<OsString>, videos: Option<PathBuf>) -> Option<PathBuf> {
    match over.map(PathBuf::from) {
        Some(path) if path.is_absolute() => Some(path),
        over => {
            if let Some(path) = over {
                // An unexpanded `~/small` would otherwise record onto the main disk unnoticed.
                tracing::warn!(
                    root = %tilde(&path.to_string_lossy()),
                    "KLIPOLO_RECORDINGS_ROOT isn't an absolute path, so it is ignored"
                );
            }
            videos.map(|v| v.join("Klipolo").join("Recordings"))
        }
    }
}

/// `text` with the user's home folder written `~` wherever it starts a path (log lines: a full path names the person's
/// account). On Windows also as Rust's `{:?}` writes it (`C:\\Users\\sam`) and with forward slashes. `text` as it
/// is without a home folder. O(len).
pub fn tilde(text: &str) -> String {
    match dirs::home_dir() {
        Some(home) => tilde_in(text, &home.to_string_lossy()),
        None => text.to_owned(),
    }
}

/// [`tilde`] for the home folder `home`.
fn tilde_in(text: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return text.to_owned();
    }
    let mut out = whole(text, home);
    if home.contains('\\') {
        out = whole(&out, &home.replace('\\', "\\\\"));
        out = whole(&out, &home.replace('\\', "/"));
    }
    out
}

/// `text` with `home` written `~` where it is the folder itself, never a longer name that starts like it (`/Users/sam2`).
fn whole(text: &str, home: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(home) {
        let after = &rest[i + home.len()..];
        let whole = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.')));
        out.push_str(&rest[..i]);
        out.push_str(if whole { "~" } else { home });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Where the app's log goes: `~/Library/Logs/Klipolo` on macOS (where Console looks), `%LOCALAPPDATA%\Klipolo\Logs` on
/// Windows, and `$XDG_STATE_HOME/klipolo/logs` on Linux (else `~/.local/state/klipolo/logs`). None: no home folder.
pub fn logs_dir() -> Option<PathBuf> {
    crate::imp::logs_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_on_windows_too() {
        assert_eq!(safe_file_name("  a/b:c\u{7}.  "), "a-b-c");
        assert_eq!(
            safe_file_name("What? <Draft> \"v2\" a\\b|c*"),
            "What- -Draft- -v2- a-b-c-"
        );
        assert_eq!(safe_file_name(" ... "), "Untitled video");
        assert_eq!(safe_file_name(&"é".repeat(300)).chars().count(), 100);
        assert_eq!(safe_file_name("con"), "con_");
        assert_eq!(safe_file_name("NUL.txt"), "NUL_.txt");
        assert_eq!(safe_file_name("Lpt9 "), "Lpt9_");
        assert_eq!(safe_file_name("COM1 (Landscape)"), "COM1 (Landscape)");
        assert_eq!(safe_file_name("Console"), "Console");
        assert!(videos_dir().is_some());
    }

    #[test]
    fn the_home_folder_is_written_as_a_tilde() {
        let home = dirs::home_dir().unwrap().display().to_string();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            tilde(&format!(
                "read {home}{sep}Models{sep}m.gguf: denied ({home})"
            )),
            format!("read ~{sep}Models{sep}m.gguf: denied (~)")
        );
        assert_eq!(tilde(&format!("{home}2{sep}x")), format!("{home}2{sep}x"));
        // Windows forms: as written, as `{:?}` escapes it, and with forward slashes.
        let win = r"C:\Users\sam";
        assert_eq!(
            tilde_in(r"read C:\Users\sam\Models\m.gguf", win),
            r"read ~\Models\m.gguf"
        );
        assert_eq!(
            tilde_in(&format!("{:?}", r"C:\Users\sam\m.gguf"), win),
            r#""~\\m.gguf""#
        );
        assert_eq!(tilde_in("C:/Users/sam/m.gguf", win), "~/m.gguf");
    }

    #[test]
    fn recordings_go_to_the_videos_folder_unless_an_absolute_root_is_set() {
        let videos = Some(PathBuf::from("/home/sam/Videos"));
        let made = PathBuf::from("/home/sam/Videos/Klipolo/Recordings");
        assert_eq!(recordings_root_in(None, videos.clone()), Some(made.clone()));
        let disk = if cfg!(windows) {
            r"D:\small"
        } else {
            "/Volumes/small"
        };
        assert_eq!(
            recordings_root_in(Some(disk.into()), videos.clone()),
            Some(PathBuf::from(disk))
        );
        assert_eq!(
            recordings_root_in(Some("rel".into()), videos.clone()),
            Some(made)
        );
        assert_eq!(recordings_root_in(Some("".into()), None), None);
    }

    #[test]
    fn the_log_goes_where_the_os_keeps_logs() {
        let logs = logs_dir().unwrap();
        #[cfg(target_os = "macos")]
        assert!(logs.ends_with("Library/Logs/Klipolo"), "{logs:?}");
        #[cfg(windows)]
        assert!(logs.ends_with("Klipolo/Logs"), "{logs:?}");
        #[cfg(target_os = "linux")]
        assert!(logs.ends_with("klipolo/logs"), "{logs:?}");
    }
}
