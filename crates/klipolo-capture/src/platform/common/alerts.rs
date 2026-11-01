//! Which on-screen windows are a system alert on macOS (`Backend::system_alert`), from what the window list says of
//! each, with no OS call. macOS's own questions are drawn by `UserNotificationCenter`: the permission prompts, and the
//! one replayd raises as a take's first stream starts after a grant, or once the last answer is a month old ("Klipolo
//! is requesting to bypass the system private window picker and directly access your screen and audio."). replayd is
//! named too, should it ever draw that question itself. A window counts while a person could see it: on the screen,
//! not see-through and big enough to read.

/// The executables whose windows are the system's questions, by file name.
pub const ALERT_OWNERS: &[&str] = &["UserNotificationCenter", "replayd"];

/// A window narrower or shorter than this (points) holds no question a person could read.
const SMALLEST_SIDE: f64 = 40.0;

/// One on-screen window as the window list gives it.
#[derive(Clone, Copy, Debug)]
pub struct ListedWindow<'a> {
    /// Its owner's executable path, when it can be read.
    pub owner: Option<&'a str>,
    /// Its size in points.
    pub size: (f64, f64),
    /// Its alpha, 0 to 1.
    pub alpha: f64,
}

/// Whether `window` is a system alert a take must not record.
pub fn is_alert(window: &ListedWindow<'_>) -> bool {
    let owner = (window.owner)
        .and_then(|path| path.rsplit('/').next())
        .is_some_and(|name| ALERT_OWNERS.contains(&name));
    owner && window.alpha > 0.0 && window.size.0 >= SMALLEST_SIDE && window.size.1 >= SMALLEST_SIDE
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTRE: &str = "/System/Library/CoreServices/UserNotificationCenter.app/Contents/MacOS/UserNotificationCenter";

    fn window(owner: Option<&str>) -> ListedWindow<'_> {
        ListedWindow {
            owner,
            size: (420.0, 260.0),
            alpha: 1.0,
        }
    }

    #[test]
    fn the_system_s_questions_are_alerts() {
        assert!(is_alert(&window(Some(CENTRE))));
        assert!(is_alert(&window(Some("/usr/libexec/replayd"))));
    }

    #[test]
    fn other_windows_are_not() {
        for owner in [
            "/Applications/Safari.app/Contents/MacOS/Safari",
            "/System/Library/CoreServices/NotificationCenter.app/Contents/MacOS/NotificationCenter",
            "/Applications/Klipolo.app/Contents/MacOS/klipolo-app",
            "/Applications/UserNotificationCenter Helper.app/Contents/MacOS/UserNotificationCenter Helper",
        ] {
            assert!(!is_alert(&window(Some(owner))), "{owner}");
        }
        assert!(!is_alert(&window(None)), "an owner that can't be read");
    }

    #[test]
    fn a_window_no_one_could_read_is_not() {
        let hidden = ListedWindow {
            alpha: 0.0,
            ..window(Some(CENTRE))
        };
        let tiny = ListedWindow {
            size: (420.0, 1.0),
            ..window(Some(CENTRE))
        };
        assert!(!is_alert(&hidden));
        assert!(!is_alert(&tiny));
    }
}
