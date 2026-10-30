//! What of a worker's stderr reaches the app's log: the export and editing-copy workers' (`launch`), the
//! recorder's (the app's `recorder::launch`) and the speech worker's (`klipolo_speech::runtime`). One rule for every
//! worker.

use std::time::{Duration, Instant};

/// Worker stderr lines forwarded into the app log per second, per job; past it they are counted, not logged (a
/// damaged source can make FFmpeg warn on every frame, which would rotate the log away mid-export).
const LINES_PER_S: u32 = 20;

/// What of a worker's stderr reaches the app log: identical consecutive lines collapse to "(repeated N times)", and
/// past `LINES_PER_S` lines in a second (summaries included) the rest are counted and the count logged.
#[derive(Default)]
pub struct Forward {
    last: Option<String>,
    /// Whether `last` reached the log; repeats of a dropped line are counted with the drops.
    kept: bool,
    repeats: u32,
    window: Option<Instant>,
    sent: u32,
    dropped: u32,
}

impl Forward {
    /// The lines to log for `line`, which arrived at `now`.
    pub fn line(&mut self, line: String, now: Instant) -> Vec<String> {
        if self.last.as_deref() == Some(line.as_str()) {
            if self.kept {
                self.repeats += 1;
            } else {
                self.dropped += 1;
            }
            return Vec::new();
        }
        let mut out = Vec::new();
        if self
            .window
            .is_none_or(|start| now.duration_since(start) >= Duration::from_secs(1))
        {
            out.extend(self.dropped());
            (self.window, self.sent) = (Some(now), 0);
        }
        if let n @ 1.. = std::mem::take(&mut self.repeats) {
            self.send(format!("(repeated {n} times)"), n, &mut out);
        }
        self.kept = self.send(line.clone(), 1, &mut out);
        self.last = Some(line);
        out
    }

    /// Logs `text` within this second's budget, or counts the `lines` it stands for as dropped.
    fn send(&mut self, text: String, lines: u32, out: &mut Vec<String>) -> bool {
        if self.sent < LINES_PER_S {
            self.sent += 1;
            out.push(text);
            true
        } else {
            self.dropped += lines;
            false
        }
    }

    /// What is still owed when the worker's stderr closes.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if let n @ 1.. = std::mem::take(&mut self.repeats) {
            out.push(format!("(repeated {n} times)"));
        }
        out.extend(self.dropped());
        out
    }

    fn dropped(&mut self) -> Option<String> {
        match std::mem::take(&mut self.dropped) {
            0 => None,
            n => Some(format!(
                "({n} lines dropped: more than {LINES_PER_S} a second)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_lines_collapse_repeats_and_are_capped_per_second() {
        let mut f = Forward::default();
        let t0 = Instant::now();
        let mut logged = Vec::new();
        for _ in 0..5 {
            logged.extend(f.line("[h264] corrupt macroblock".into(), t0));
        }
        logged.extend(f.line("export set-up 0.40 s".into(), t0));
        assert_eq!(
            logged,
            [
                "[h264] corrupt macroblock",
                "(repeated 4 times)",
                "export set-up 0.40 s"
            ]
        );
        // 100 different lines in one second: 20 a second reach the log (the summary above counted), the rest are
        // counted.
        let burst: Vec<String> = (0..100)
            .flat_map(|n| f.line(format!("warning {n}"), t0))
            .collect();
        assert_eq!(burst.len(), (LINES_PER_S - 3) as usize);
        // Repeats of a dropped line are drops, never "(repeated …)" after the last kept line; alternating pairs of
        // repeats stay under the cap.
        for n in 0..50 {
            for _ in 0..2 {
                assert!(f.line(format!("pair {}", n % 2), t0).is_empty());
            }
        }
        let next = f.line("export done 12.00 s".into(), t0 + Duration::from_secs(1));
        assert_eq!(
            next,
            [
                "(183 lines dropped: more than 20 a second)",
                "export done 12.00 s"
            ]
        );
        assert!(f.finish().is_empty());
    }
}
