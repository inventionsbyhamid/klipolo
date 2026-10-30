//! The preview's note for a disk that doesn't answer. A frame whose source's disk hasn't answered within
//! the pool's stat limit is drawn without it and says nothing (`Rendered::not_answering`): a disk spinning up answers
//! in a moment, and the pool's redraw signal draws the frame again then. One that stays silent for `WAITING_AFTER`
//! gets a neutral note under the preview (`api::WAITING_FOR_DISK`, "Waiting for the disk with intro.mp4…"), which goes
//! as soon as a frame is drawn without waiting for it: its disk answered, or the preview no longer needs it.
//!
//! Times are the runtime's (`tokio::time::Instant`), so the note's timer and its check read the same clock.

use std::sync::Arc;

use tokio::time::Instant;

use crate::api::{CodedError, WAITING_AFTER, waiting_notice};

/// Which sources the latest frame waited for, since when, and the one the note names.
#[derive(Default)]
pub(super) struct Waiting {
    /// The sources the latest frame waited for, with when a frame first did, oldest first (a handful at most: the
    /// video layers of one frame).
    silent: Vec<(Arc<str>, Instant)>,
    /// The source the note names, while it shows.
    said: Option<Arc<str>>,
}

/// A change of the note: the note to show, or None to take it away.
pub(super) type NoteChange = Option<Option<CodedError>>;

impl Waiting {
    /// A frame was drawn waiting for `paths` (empty: it waited for none). A source it no longer waits for is
    /// forgotten; a new one starts its wait now. O(paths × silent), both a handful.
    pub(super) fn frame(&mut self, paths: &[Arc<str>], now: Instant) -> NoteChange {
        if paths.is_empty() && self.silent.is_empty() {
            return None;
        }
        self.silent.retain(|(path, _)| paths.contains(path));
        for path in paths {
            if !self.silent.iter().any(|(p, _)| p == path) {
                self.silent.push((path.clone(), now));
            }
        }
        self.update(now)
    }

    /// The note due now: the source silent longest, once that is `WAITING_AFTER`, else none.
    pub(super) fn update(&mut self, now: Instant) -> NoteChange {
        let due = self
            .silent
            .first()
            .filter(|(_, since)| now.duration_since(*since) >= WAITING_AFTER)
            .map(|(path, _)| path.clone());
        if due == self.said {
            return None;
        }
        self.said = due;
        Some(self.said.as_deref().map(waiting_notice))
    }

    /// When the note becomes due, while it isn't shown and a frame waits; None otherwise (no timer).
    pub(super) fn due(&self) -> Option<Instant> {
        if self.said.is_some() {
            return None;
        }
        self.silent.first().map(|(_, since)| *since + WAITING_AFTER)
    }

    /// Everything forgotten (another project): the note goes if it showed.
    pub(super) fn clear(&mut self) -> NoteChange {
        self.silent.clear();
        self.said.take().map(|_| None)
    }
}

/// Resolves when the note becomes due; never without a wait to time.
pub(super) async fn waiting_due(due: Option<Instant>) {
    match due {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_source_silent_long_enough_is_named_until_a_frame_draws_without_it() {
        let mut w = Waiting::default();
        let start = Instant::now();
        let (a, b): (Arc<str>, Arc<str>) = ("/Volumes/Archive/intro.mp4".into(), "/b.mov".into());
        assert_eq!(
            w.frame(std::slice::from_ref(&a), start),
            None,
            "a disk spinning up is never said"
        );
        assert_eq!(w.due(), Some(start + WAITING_AFTER));
        // Still silent a moment later: its wait runs from the first frame.
        let later = start + Duration::from_secs(4);
        assert_eq!(w.frame(&[a.clone(), b.clone()], later), None);
        let note = w.update(start + WAITING_AFTER).expect("due");
        assert_eq!(note, Some(waiting_notice(&a)));
        assert_eq!(note.unwrap().params["name"], "intro.mp4".into());
        assert_eq!(w.due(), None, "shown: no timer");
        // Its disk answered, the other still waits but not long enough: the note goes, and the other's timer runs.
        assert_eq!(
            w.frame(std::slice::from_ref(&b), start + WAITING_AFTER),
            Some(None)
        );
        assert_eq!(w.due(), Some(later + WAITING_AFTER));
        assert_eq!(w.clear(), None, "nothing shown");
        assert_eq!(w.frame(&[], later), None);
    }
}
