//! The recorder tests' fake worker: a pair of in-memory pipes the test drives. Its stdout gives the lines the test
//! emits, then the end with the exit code the test (or a hard stop) gives; its stdin collects the commands the recorder
//! sends. No process starts.

use std::io::{self, BufRead, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use klipolo_capture::protocol::{Command, Event};

use super::launch::{Launch, WorkerIo};

/// One fake worker's far ends.
struct Far {
    /// Its stdout: lines the test emits; None once closed.
    lines: Option<Sender<Vec<u8>>>,
    code: Option<i32>,
}

#[derive(Default)]
pub(super) struct Launched {
    spawned: Mutex<Vec<Vec<String>>>,
    workers: Mutex<Vec<Arc<Mutex<Far>>>>,
    commands: Arc<Mutex<Vec<Command>>>,
    kills: Mutex<Vec<bool>>,
    /// Lines and exits sent to the recorder (`Fixture::settle` waits for it to handle as many).
    pub(super) sent: AtomicU64,
}

/// Launches fake workers: each is a pair of in-memory pipes the test drives.
#[derive(Clone, Default)]
pub(super) struct FakeLaunch(pub(super) Arc<Launched>);

impl FakeLaunch {
    pub(super) fn spawned(&self) -> usize {
        self.0.spawned.lock().unwrap().len()
    }

    pub(super) fn last_args(&self) -> Vec<String> {
        (self.0.spawned.lock().unwrap().last())
            .cloned()
            .unwrap_or_default()
    }

    /// Every command the workers were sent, in order.
    pub(super) fn commands(&self) -> Vec<Command> {
        self.0.commands.lock().unwrap().clone()
    }

    pub(super) fn kills(&self) -> Vec<bool> {
        self.0.kills.lock().unwrap().clone()
    }

    fn last(&self) -> Arc<Mutex<Far>> {
        (self.0.workers.lock().unwrap().last())
            .cloned()
            .expect("a worker was launched")
    }

    /// The latest worker says `ev`.
    pub(super) fn emit(&self, ev: Event) {
        let mut line = serde_json::to_vec(&ev).unwrap();
        line.push(b'\n');
        let far = self.last();
        let far = far.lock().unwrap();
        (far.lines.as_ref())
            .expect("the worker is running")
            .send(line)
            .unwrap();
        self.0.sent.fetch_add(1, Ordering::SeqCst);
    }

    /// The latest worker ends with `code`.
    pub(super) fn exit(&self, code: Option<i32>) {
        end(&self.last(), code, &self.0.sent);
    }
}

/// Closes a fake worker's stdout with its exit `code`: one more thing for the recorder to handle.
fn end(far: &Mutex<Far>, code: Option<i32>, sent: &AtomicU64) {
    let mut far = far.lock().unwrap();
    if far.lines.take().is_some() {
        far.code = code;
        sent.fetch_add(1, Ordering::SeqCst);
    }
}

impl Launch for FakeLaunch {
    fn spawn(&self, args: &[String]) -> io::Result<WorkerIo> {
        self.0.spawned.lock().unwrap().push(args.to_vec());
        let (tx, rx) = channel();
        let far = Arc::new(Mutex::new(Far {
            lines: Some(tx),
            code: None,
        }));
        self.0.workers.lock().unwrap().push(far.clone());
        let (killed, launched) = (far.clone(), self.0.clone());
        Ok(WorkerIo {
            stdin: Box::new(Commands {
                to: self.0.commands.clone(),
                line: Vec::new(),
            }),
            stdout: Box::new(Lines {
                rx,
                buf: Vec::new(),
                at: 0,
            }),
            pid: 4_242,
            // A hung worker ignores the request; the hard stop ends it as a signal does.
            kill: Box::new(move |hard| {
                launched.kills.lock().unwrap().push(hard);
                if hard {
                    end(&killed, None, &launched.sent);
                }
            }),
            wait: Box::new(move || far.lock().unwrap().code),
        })
    }
}

/// A fake worker's stdin: the commands it reads.
struct Commands {
    to: Arc<Mutex<Vec<Command>>>,
    line: Vec<u8>,
}

impl Write for Commands {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(buf);
        while let Some(at) = self.line.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.line.drain(..=at).collect();
            let cmd = serde_json::from_slice(&line).expect("a command line");
            self.to.lock().unwrap().push(cmd);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A fake worker's stdout: the test's lines, then the end once it exits.
struct Lines {
    rx: Receiver<Vec<u8>>,
    buf: Vec<u8>,
    at: usize,
}

impl Read for Lines {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = {
            let have = self.fill_buf()?;
            let n = have.len().min(out.len());
            out[..n].copy_from_slice(&have[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Lines {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.at == self.buf.len() {
            self.buf = self.rx.recv().unwrap_or_default();
            self.at = 0;
        }
        Ok(&self.buf[self.at..])
    }

    fn consume(&mut self, n: usize) {
        self.at += n;
    }
}
