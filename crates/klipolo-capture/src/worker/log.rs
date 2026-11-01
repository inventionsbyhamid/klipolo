//! The worker's log: a take's goes to its folder's `diagnostics.log`, at most `MAX_BYTES`, then moved to
//! `diagnostics.1.log` once (the older one replaced) so the record never grows without bound; the other forms log to
//! stderr (stdout carries the protocol). Installing is best effort: a log that can't open never stops a take.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A log file this big is rotated.
const MAX_BYTES: u64 = 5 << 20;

pub(super) fn to_stderr() {
    let _ = tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_ansi(false)
        .try_init();
}

pub(super) fn to_file(path: &Path) {
    match Rotating::open(path) {
        Ok(file) => {
            let _ = tracing_subscriber::fmt()
                .with_writer(Mutex::new(file))
                .with_ansi(false)
                .try_init();
        }
        Err(e) => {
            to_stderr();
            tracing::warn!(error = %e, "worker: no diagnostics.log; logging to stderr");
        }
    }
}

/// An append-only log file rotated once at `MAX_BYTES`.
struct Rotating {
    path: PathBuf,
    file: File,
    len: u64,
}

impl Rotating {
    fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            file,
            len,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        std::fs::rename(&self.path, self.path.with_extension("1.log"))?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.len = 0;
        Ok(())
    }
}

impl Write for Rotating {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len + buf.len() as u64 > MAX_BYTES && self.len > 0 {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
