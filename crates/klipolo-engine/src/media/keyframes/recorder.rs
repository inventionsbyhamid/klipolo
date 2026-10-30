//! Which keyframes a thumbnail pass has seen across its runs, and whether they are all of the file's.

use std::collections::BTreeSet;

/// One run of packets: the keyframes it saw, whether it began at the file's first packet and ended at its last.
#[derive(Clone, Copy, Default)]
struct Run {
    first: Option<i64>,
    last: Option<i64>,
    from_start: bool,
    eof: bool,
}

/// Keyframes seen across a pass's runs (each run reads every keyframe from where it started), and whether they are
/// all of the file's: runs that chain from the start to the end, each starting at or before where the last one
/// reached.
#[derive(Default)]
pub(crate) struct KeyRecorder {
    keys: BTreeSet<i64>,
    runs: Vec<Run>,
    current: Option<Run>,
}

impl KeyRecorder {
    pub(crate) fn begin(&mut self, from_start: bool) {
        self.end(false);
        self.current = Some(Run {
            from_start,
            ..Run::default()
        });
    }

    pub(crate) fn key(&mut self, pts: i64) {
        if let Some(run) = &mut self.current {
            run.first.get_or_insert(pts);
            run.last = Some(run.last.map_or(pts, |last| last.max(pts)));
            self.keys.insert(pts);
        }
    }

    pub(crate) fn end(&mut self, eof: bool) {
        if let Some(mut run) = self.current.take() {
            run.eof = eof;
            self.runs.push(run);
        }
    }

    /// Every keyframe of the file has been seen.
    pub(crate) fn complete(&self) -> bool {
        let mut reach: Option<i64> = None;
        let mut started = false;
        for run in self.runs.iter().filter(|r| r.from_start) {
            if run.eof {
                return true;
            }
            started = true;
            reach = reach.max(run.last);
        }
        let Some(mut reach) = reach.filter(|_| started) else {
            return false;
        };
        loop {
            let mut grew = false;
            for run in &self.runs {
                let (Some(first), Some(last)) = (run.first, run.last) else {
                    continue;
                };
                if first > reach {
                    continue;
                }
                if run.eof {
                    return true;
                }
                if last > reach {
                    reach = last;
                    grew = true;
                }
            }
            if !grew {
                return false;
            }
        }
    }

    pub(crate) fn pts(&self) -> Vec<i64> {
        self.keys.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_that_chain_from_the_start_to_the_end_see_every_keyframe() {
        // The playhead's run first (45 s to the end), then the start up to it: they overlap at a keyframe.
        let mut r = KeyRecorder::default();
        r.begin(false);
        [43, 44, 100].into_iter().for_each(|k| r.key(k));
        r.end(true);
        r.begin(true);
        [0, 1, 43].into_iter().for_each(|k| r.key(k));
        r.end(false);
        assert!(r.complete());
        // A run that starts past where the start run reached may have missed one in between.
        let mut gap = KeyRecorder::default();
        gap.begin(true);
        [0, 1].into_iter().for_each(|k| gap.key(k));
        gap.begin(false);
        [5, 9].into_iter().for_each(|k| gap.key(k));
        gap.end(true);
        assert!(!gap.complete());
        assert_eq!(r.pts(), [0, 1, 43, 44, 100]);
    }
}
