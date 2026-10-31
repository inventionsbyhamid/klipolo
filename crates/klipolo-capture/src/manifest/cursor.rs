//! The manifest's cursor, from the config and the journal alone, as the rest of the
//! manifest: the screen's plan, the first `CursorBurned` (else the watchdog's `CursorBurnDecided`) as the time the
//! frames show it from, and the weakest click mode the journal holds (`CursorClicks`: the start's, then a switch to
//! sampling). A take whose journal has no click mode (one cut short before its first tick) says sampled.

use klipolo_cursor::ClickSource;
use klipolo_model::cursor_style::CursorCapture;

use super::{ManifestCursor, ManifestSource};
use crate::journal::{Entry, JournalRead};
use crate::preflight::cursor::CursorPlan;
use crate::session::{CURSOR, SessionConfig, SourceSpec};

/// The cursor of a take whose screen recorded one and has runs; None otherwise (a game whose profile has no cursor).
pub(super) fn cursor_of(
    cfg: &SessionConfig,
    journal: &JournalRead,
    sources: &[ManifestSource],
) -> Option<ManifestCursor> {
    let plan = cfg.sources.iter().find_map(|s| match &s.spec {
        SourceSpec::Screen(v)
            if v.cursor != CursorPlan::None && sources.iter().any(|m| m.id == s.id) =>
        {
            Some(v.cursor)
        }
        _ => None,
    })?;
    let (mut decided, mut burned, mut clicks) = (None, None, None::<ClickSource>);
    for entry in &journal.entries {
        match *entry {
            Entry::CursorBurnDecided { at_us } => {
                decided.get_or_insert(at_us);
            }
            Entry::CursorBurned { at_us } => {
                burned.get_or_insert(at_us);
            }
            Entry::CursorClicks { clicks: c, .. } => clicks = Some(clicks.map_or(c, |w| w.max(c))),
            _ => {}
        }
    }
    let capture = match (plan, burned.or(decided)) {
        (CursorPlan::Separate, Some(at_us)) => CursorCapture::BurnedFrom { at_us },
        (CursorPlan::Separate, None) => CursorCapture::Separate,
        (CursorPlan::Burned | CursorPlan::None, _) => CursorCapture::Burned,
    };
    Some(ManifestCursor {
        file: CURSOR.to_owned(),
        capture,
        clicks: clicks.unwrap_or(ClickSource::Sampled),
    })
}
