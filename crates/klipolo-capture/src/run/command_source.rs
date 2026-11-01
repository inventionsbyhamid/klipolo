//! Where a worker reads the app's commands (`CommandSource`), and the fence (`CommandFence`) that lets a resumed take
//! decide whether to record only once every command the app had already sent is read and handled. So a Stop waiting in
//! the worker's input is always seen before anything opens, however late the reader's thread runs on a busy computer:
//! the order is kept by the fence, never by timing.
//!
//! The reader reads only under the fence, and there only what already waits (a pipe's readiness, `poll`; all of a
//! buffer in memory), so outside the fence nothing it read is half handled. The decision, under the fence, waits while
//! bytes still wait. Waiting for bytes is done outside the fence, so a reader with nothing to read never holds it.

use std::io::{BufRead, BufReader, Cursor, Read};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// The app's commands as a worker reads them: lines, and how to tell whether more already wait.
pub trait CommandSource: BufRead + Send {
    /// How the worker tells whether bytes the app sent wait to be read.
    fn waiting(&self) -> Waiting;
    /// Whether bytes are buffered already (read from the source, not yet taken as lines).
    fn buffered(&self) -> bool;
}

/// How a worker tells whether bytes the app sent wait in its input.
#[derive(Clone, Copy, Debug)]
pub enum Waiting {
    /// A pipe: bytes wait while its descriptor is readable (`klipolo_platform::process::readable`).
    #[cfg(unix)]
    Pipe(std::os::fd::RawFd),
    /// A buffer in memory: everything sent is in it, read to its end at once.
    Memory,
    /// No way to tell (a pipe on Windows): a decision takes what the reader has read so far.
    Unknown,
}

#[cfg(unix)]
impl<R: Read + Send + std::os::fd::AsRawFd> CommandSource for BufReader<R> {
    fn waiting(&self) -> Waiting {
        Waiting::Pipe(self.get_ref().as_raw_fd())
    }

    fn buffered(&self) -> bool {
        !self.buffer().is_empty()
    }
}

#[cfg(not(unix))]
impl<R: Read + Send> CommandSource for BufReader<R> {
    fn waiting(&self) -> Waiting {
        Waiting::Unknown
    }

    fn buffered(&self) -> bool {
        !self.buffer().is_empty()
    }
}

impl CommandSource for Cursor<Vec<u8>> {
    fn waiting(&self) -> Waiting {
        Waiting::Memory
    }

    fn buffered(&self) -> bool {
        self.position() < self.get_ref().len() as u64
    }
}

/// The fence between the command reader and a resumed take's decision (the module doc).
pub(super) struct CommandFence {
    waiting: Waiting,
    /// The input ended (its end, or a read that failed): nothing more will come.
    ended: Mutex<bool>,
    changed: Condvar,
}

impl CommandFence {
    pub(super) fn new(waiting: Waiting) -> Self {
        Self {
            waiting,
            ended: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    /// Whether more of what the app sent waits: bytes `buffered` already, or waiting in the source.
    pub(super) fn more(&self, buffered: bool) -> bool {
        buffered
            || match self.waiting {
                #[cfg(unix)]
                Waiting::Pipe(fd) => klipolo_platform::process::readable(fd, false),
                Waiting::Memory => true,
                Waiting::Unknown => false,
            }
    }

    /// The reader, outside the fence: blocks until bytes wait in a pipe (unless some are `buffered`); the other sources
    /// are read at once.
    pub(super) fn wait_for_bytes(&self, buffered: bool) {
        #[cfg(unix)]
        if let Waiting::Pipe(fd) = self.waiting
            && !buffered
        {
            klipolo_platform::process::readable(fd, true);
        }
        #[cfg(not(unix))]
        let _ = buffered;
    }

    /// The reader: `read` reads and handles every line that waits, under the fence, and says whether the input ended.
    pub(super) fn reading<T>(&self, read: impl FnOnce() -> (T, bool)) -> T {
        let mut ended = self.lock();
        let (out, end) = read();
        *ended |= end;
        drop(ended);
        self.changed.notify_all();
        out
    }

    /// The decision: once everything the app had already sent is read and handled (or the input ended), `decide` runs
    /// under the fence, before the reader reads anything more. With no way to tell, it runs at once.
    pub(super) fn settled<T>(&self, decide: impl FnOnce() -> T) -> T {
        if matches!(self.waiting, Waiting::Unknown) {
            return decide();
        }
        let mut ended = self.lock();
        while !*ended && self.more(false) {
            ended = (self.changed.wait(ended)).unwrap_or_else(PoisonError::into_inner);
        }
        decide()
    }

    fn lock(&self) -> MutexGuard<'_, bool> {
        self.ended.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn a_decision_waits_until_a_buffer_in_memory_is_read_to_its_end() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let fence = Arc::new(CommandFence::new(Waiting::Memory));
        let (reader, done) = (fence.clone(), Arc::new(AtomicBool::new(false)));
        let reading = done.clone();
        let read = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            reader.reading(|| {
                reading.store(true, Ordering::Release);
                ((), true)
            });
        });
        let ended = fence.settled(|| done.load(Ordering::Acquire));
        assert!(ended, "decided only once the reader reached the end");
        read.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_decision_waits_while_bytes_wait_in_a_pipe() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let (pipe, mut app) = std::io::pipe().unwrap();
        let fence = CommandFence::new(Waiting::Pipe(pipe.as_raw_fd()));
        // Nothing sent yet: decided at once.
        assert!(!fence.more(false));
        fence.settled(|| ());
        app.write_all(b"{\"cmd\":\"stop\"}\n").unwrap();
        assert!(fence.more(false), "the Stop waits");
        let fence = Arc::new(fence);
        let reader = fence.clone();
        let mut input = BufReader::new(pipe);
        let read = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            reader.wait_for_bytes(false);
            let line = reader.reading(|| {
                let mut line = String::new();
                input.read_line(&mut line).unwrap();
                (line, false)
            });
            // The pipe stays open until the test is done with it: a closed one would read as waiting.
            (line, input)
        });
        // Decided only once the reader took the Stop: nothing waits any more.
        let waited = fence.settled(|| !fence.more(false));
        assert!(waited);
        assert_eq!(read.join().unwrap().0, "{\"cmd\":\"stop\"}\n");
    }
}
