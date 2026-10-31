//! Stop's last steps on the core thread (`run::threads`'s module doc): the writer's `finish`, then `finish_take`, on a
//! thread of their own and watched for a drive that stopped answering; heartbeats go on meanwhile. A take that couldn't
//! be finished still gets its Stop line on the disk, best effort.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};

use super::driver::Core;
use super::effect::Effect;
use super::finish::finish_take;
use super::threads::{STUCK, TICK, failed_with, spawn};
use crate::journal;
use crate::protocol::Event;
use crate::writer::WriterProgress;

/// How long an unfinished Stop waits for its own write of the Stop line (`stop_line_on_disk`).
const STOP_LINE_WAIT: Duration = Duration::from_secs(2);

impl Core {
    /// Stop's last steps (the module doc): the writer's `finish`, then `finish_take`, watched for a drive that stopped
    /// answering; heartbeats go on meanwhile.
    pub(super) fn finalize(&mut self) -> Vec<Effect> {
        self.pipes.stop_all();
        let (runs, earlier) = self.core.take_done();
        let Some(writer) = self.writer.take() else {
            return self.core.finished(Err(failed_with("no writer")), host_ns());
        };
        let progress = writer.progress();
        let steps = Arc::new(AtomicU64::new(0));
        let (tx, rx) = channel();
        let (io, dir, cfg, counted) = (
            self.io.clone(),
            self.dir.clone(),
            self.cfg.clone(),
            steps.clone(),
        );
        spawn("klipolo-finish", move || {
            qos::set_current(Qos::UserInitiated);
            let done = writer
                .finish()
                .and_then(|snap| finish_take(&*io, &dir, &cfg, &runs, &earlier, &snap, &counted));
            let _ = tx.send(done.map(|_| ()).map_err(|e| e.to_string()));
        });
        let mut seen = (progress.bytes(), steps.load(Ordering::Relaxed));
        let mut idle_since = Instant::now();
        let result = loop {
            match rx.recv_timeout(TICK) {
                Ok(done) => break done.map_err(|e| failed_with(&e)),
                Err(RecvTimeoutError::Disconnected) => {
                    break Err(failed_with("the finishing thread ended"));
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.beats += 1;
                    self.emit(Event::Heartbeat {
                        n: self.beats,
                        queued_ms: 0,
                        free_bytes: None,
                    });
                    let now = (progress.bytes(), steps.load(Ordering::Relaxed));
                    if now != seen {
                        (seen, idle_since) = (now, Instant::now());
                    } else if idle_since.elapsed() >= STUCK {
                        tracing::error!(
                            "core: the drive stopped answering while the take was finished"
                        );
                        break Err(failed_with("the drive stopped answering"));
                    }
                }
            }
        };
        if let Err(e) = &result {
            tracing::error!(error = ?e, "core: the take couldn't be finished; recovery will");
            self.stop_line_on_disk(&progress);
        }
        self.core.finished(result, host_ns())
    }

    /// The take couldn't be finished (the drive stopped answering, or the writer failed): if the Stop line didn't reach
    /// the journal, it is appended here, outside the writer, on a thread of its own and within `STOP_LINE_WAIT`, so a
    /// resume never records after this Stop. Best effort: a dead drive takes nothing.
    fn stop_line_on_disk(&mut self, progress: &WriterProgress) {
        let Some((line, taken)) = self.stop_line.take() else {
            return;
        };
        if taken && progress.urgent_pending() == 0 {
            return;
        }
        // The writer may still wake: it must never cut this line off with a retry's truncation.
        progress.abandon();
        let line = apart(&line);
        let path = self.dir.journal();
        let (tx, rx) = channel();
        spawn("klipolo-stop-line", move || {
            let written = journal::open_append(&path).and_then(|mut file| {
                std::io::Write::write_all(&mut file, &line)?;
                file.sync_data()
            });
            let _ = tx.send(written);
        });
        match rx.recv_timeout(STOP_LINE_WAIT) {
            Ok(Ok(())) => tracing::info!("core: the Stop line was written outside the writer"),
            Ok(Err(e)) => tracing::error!(error = %e, "core: the Stop line couldn't be written"),
            Err(_) => tracing::error!("core: the Stop line didn't reach the drive in time"),
        }
    }
}

/// A journal line for a second handle, while the writer's own may still be mid-write: it starts with a newline, in the
/// same write, so it never glues onto a line the writer left half-written after `open_append`'s check.
/// The reader skips the empty line this may leave.
fn apart(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len() + 1);
    out.push(b'\n');
    out.extend_from_slice(line);
    out
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::apart;
    use crate::journal::{self, Entry};

    #[test]
    fn a_stop_line_written_apart_survives_a_half_written_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        std::fs::write(&path, journal::line(&Entry::Gate { host_ns: 1 })).unwrap();
        let mut second = journal::open_append(&path).unwrap();
        // The writer's short write lands after the second handle's torn-tail check.
        let mut writer = journal::open_append(&path).unwrap();
        writer.write_all(b"{\"durable\":{\"scr").unwrap();
        let stop = Entry::StopRequested {
            at_us: 5,
            reason: None,
        };
        second.write_all(&apart(&journal::line(&stop))).unwrap();
        let read = journal::read(&path).unwrap();
        assert_eq!(read.entries, vec![Entry::Gate { host_ns: 1 }, stop]);
        assert_eq!(read.skipped, 1, "only the half line is lost");
    }
}
