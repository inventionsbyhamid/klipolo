//! Which on-screen windows are a system alert on macOS (`Backend::system_alert`), from what the window list says of
//! each, with no OS call. macOS's own questions are drawn by `UserNotificationCenter`: the permission prompts, and the
//! one replayd raises as a take's first stream starts after a grant, or once the last answer is a month old ("Klipolo
//! is requesting to bypass the system private window picker and directly access your screen and audio."). replayd is
//! named too, should it ever draw that question itself. A window counts while a person could see it: above the apps'
//! windows, not see-through and big enough to read. Only the windows above the normal layer are looked at, so the
//! read stays cheap at ten a second.
//!
//! What the take's log says (`AlertLog`), so a Mac run shows which process drew an alert: the windows above the normal
//! layer at the first read and whenever they change, and the alert's owner each time one comes.

/// The executables whose windows are the system's questions, by file name.
pub const ALERT_OWNERS: &[&str] = &["UserNotificationCenter", "replayd"];

/// A window narrower or shorter than this (points) holds no question a person could read.
const SMALLEST_SIDE: f64 = 40.0;

/// One on-screen window as the window list gives it.
#[derive(Clone, Copy, Debug)]
pub struct ListedWindow<'a> {
    /// Its owner's executable path, when it can be read.
    pub owner: Option<&'a str>,
    /// Its owner's process.
    pub pid: i32,
    /// Its layer: 0 is the apps' own windows; menus, panels and the system's alerts are above it.
    pub layer: i64,
    /// Its size in points.
    pub size: (f64, f64),
    /// Its alpha, 0 to 1.
    pub alpha: f64,
}

/// Whether a window at `layer` is looked at (its owner read): the system's questions float above the apps' windows.
pub fn above_normal(layer: i64) -> bool {
    layer > 0
}

/// Whether `window` is a system alert a take must not record.
pub fn is_alert(window: &ListedWindow<'_>) -> bool {
    let owner = (window.owner)
        .map(file_name)
        .is_some_and(|name| ALERT_OWNERS.contains(&name));
    owner
        && above_normal(window.layer)
        && window.alpha > 0.0
        && window.size.0 >= SMALLEST_SIDE
        && window.size.1 >= SMALLEST_SIDE
}

/// The file name at the end of `path` ("UserNotificationCenter").
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// What a take's log says about the system's windows while its countdown watches for an alert (the module doc).
#[derive(Debug, Default)]
pub struct AlertLog {
    /// The last read's windows above the normal layer, by owner and layer.
    last: Option<Vec<(String, i64)>>,
    /// An alert was on the screen at the last read.
    up: bool,
}

impl AlertLog {
    /// `windows` (those above the normal layer), as one log line, at the first read and when their owners or layers
    /// differ from the last read's: "UserNotificationCenter layer 8 pid 512 /System/Library/...". None otherwise.
    pub fn windows(&mut self, windows: &[ListedWindow<'_>]) -> Option<String> {
        let mut key: Vec<(String, i64)> = (windows.iter())
            .map(|w| (w.owner.map_or("?", file_name).to_owned(), w.layer))
            .collect();
        key.sort();
        key.dedup();
        if self.last.as_ref() == Some(&key) {
            return None;
        }
        self.last = Some(key);
        if windows.is_empty() {
            return Some("none".into());
        }
        let lines: Vec<String> = (windows.iter())
            .map(|w| {
                let owner = w.owner.unwrap_or("?");
                let (width, height) = w.size;
                format!(
                    "{} layer {} pid {} {width:.0} x {height:.0} alpha {:.2} {owner}",
                    file_name(owner),
                    w.layer,
                    w.pid,
                    w.alpha
                )
            })
            .collect();
        Some(lines.join("; "))
    }

    /// The owner's file name of the alert found now (`alert`), when none was up at the last read. None otherwise.
    pub fn alert(&mut self, alert: Option<&ListedWindow<'_>>) -> Option<String> {
        let came = alert.is_some() && !self.up;
        self.up = alert.is_some();
        came.then(|| {
            alert
                .and_then(|w| w.owner)
                .map_or("?", file_name)
                .to_owned()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTRE: &str = "/System/Library/CoreServices/UserNotificationCenter.app/Contents/MacOS/UserNotificationCenter";
    const DOCK: &str = "/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock";

    fn window(owner: Option<&str>) -> ListedWindow<'_> {
        ListedWindow {
            owner,
            pid: 512,
            layer: 8,
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

    #[test]
    fn only_windows_above_the_apps_are_looked_at() {
        assert!(!above_normal(0) && !above_normal(-20));
        assert!(above_normal(3) && above_normal(25));
        let normal = ListedWindow {
            layer: 0,
            ..window(Some(CENTRE))
        };
        assert!(!is_alert(&normal), "a window at the apps' layer");
    }

    #[test]
    fn the_log_lists_the_windows_at_the_first_read_and_when_they_change() {
        let mut log = AlertLog::default();
        let dock = ListedWindow {
            owner: Some(DOCK),
            pid: 99,
            layer: 20,
            size: (1440.0, 70.0),
            alpha: 1.0,
        };
        assert_eq!(
            log.windows(&[dock]).as_deref(),
            Some(format!("Dock layer 20 pid 99 1440 x 70 alpha 1.00 {DOCK}").as_str())
        );
        assert_eq!(log.windows(&[dock]), None, "the same windows: said once");
        let moved = ListedWindow {
            size: (1440.0, 64.0),
            pid: 100,
            ..dock
        };
        assert_eq!(log.windows(&[moved]), None, "the same owners and layers");
        let said = log.windows(&[dock, window(Some(CENTRE))]).unwrap();
        assert!(
            said.contains("UserNotificationCenter layer 8 pid 512"),
            "{said}"
        );
        assert_eq!(log.windows(&[]).as_deref(), Some("none"));
    }

    #[test]
    fn the_log_names_the_alert_s_owner_each_time_one_comes() {
        let mut log = AlertLog::default();
        let centre = window(Some(CENTRE));
        assert_eq!(log.alert(None), None);
        assert_eq!(
            log.alert(Some(&centre)).as_deref(),
            Some("UserNotificationCenter")
        );
        assert_eq!(log.alert(Some(&centre)), None, "still up: said once");
        assert_eq!(log.alert(None), None);
        assert_eq!(
            log.alert(Some(&window(Some("/usr/libexec/replayd"))))
                .as_deref(),
            Some("replayd")
        );
    }
}
