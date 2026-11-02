//! The page's perf readout, for measuring the editor on a Mac without the web inspector: launched with
//! `KLIPOLO_PAGE_PERF=1`, the page is told so before it loads (`window.__KLIPOLO_PAGE_PERF__`) and measures what a big
//! project costs it (the first render of a project, frames while the timeline scrolls and zooms, renders a second while
//! playing), and its lines come here, into the app's log under `page_perf`. Off unless the variable is set: the page
//! then measures nothing and sends nothing.

use tauri::Runtime;
use tauri::plugin::TauriPlugin;

/// The variable that turns the readout on, set to `1` at launch.
pub(crate) const VARIABLE: &str = "KLIPOLO_PAGE_PERF";

/// What the page reads, before its own scripts run, to know the readout is on.
const PAGE_FLAG: &str = "window.__KLIPOLO_PAGE_PERF__ = true;";

/// The longest line kept from the page.
const MAX_LINE: usize = 600;

fn on_from(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Whether this launch has the readout on.
pub(crate) fn on() -> bool {
    on_from(std::env::var(VARIABLE).ok().as_deref())
}

/// Tells the page the readout is on before it loads; does nothing when it is off.
pub(crate) fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let builder = tauri::plugin::Builder::new("klipolo-page-perf");
    if on() {
        builder.js_init_script(PAGE_FLAG).build()
    } else {
        builder.build()
    }
}

/// `line` as one log line: control characters as spaces, at most `MAX_LINE` characters.
fn one_line(line: &str) -> String {
    line.chars()
        .take(MAX_LINE)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A line of the page's readout, into the app's log; ignored unless the readout is on.
#[tauri::command]
pub(crate) fn page_perf_log(line: String) {
    if on() {
        tracing::info!(target: "page_perf", "{}", one_line(&line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_turns_it_on() {
        assert!(on_from(Some("1")));
        for off in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
            assert!(!on_from(off), "{off:?}");
        }
    }

    #[test]
    fn a_line_stays_one_short_line() {
        assert_eq!(one_line("scroll\nframes 12\t3"), "scroll frames 12 3");
        let long = "x".repeat(MAX_LINE + 50);
        assert_eq!(one_line(&long).chars().count(), MAX_LINE);
    }
}
