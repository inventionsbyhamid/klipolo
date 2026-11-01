//! The file steps of the durable writer (`writer`), of recovery (`recover`) and of finishing a run (the soft remux,
//! `fmp4::finalize`, and the CAF's size, `caf::finalize`), behind `Io` so tests log every step and inject a full disk,
//! an I/O error or a slow drive (`LoggedIo`, feature `synthetic`), and replay a power cut at any of them. `OsIo` does
//! them for real. (Recovery's `.damaged` copies and the manifest are written atomically beside it, not through `Io`.)

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::Path;

use klipolo_platform::durable::{self, Durability};
use klipolo_platform::file::PositionedIo;

/// What the durable writer does to files. Each step also gets the file's path, for the log.
pub trait Io: Send + Sync + 'static {
    /// A new file to append to: `create_new`, so a file already there is never reused (AlreadyExists); its folders are
    /// made, and the new entries synced (Linux), so the file survives a power cut.
    fn create(&self, path: &Path) -> io::Result<File>;
    /// An existing line file (the journal, `timing.jsonl`, `cursor.jsonl`) to append to, made if missing, through
    /// `journal::open_append`: a line a crash left torn is ended first, so the next one isn't glued to it and lost.
    fn open_append(&self, path: &Path) -> io::Result<File>;
    /// All of `bytes` at the end of `file`. After an error, part of them may be in the file.
    fn append(&self, file: &mut File, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// All of `bytes` at `at`, over what is there or past the end: a rebuilt head, the remux's tail and flips, a CAF's
    /// size.
    fn write_at(&self, file: &File, path: &Path, at: u64, bytes: &[u8]) -> io::Result<()>;
    /// Cuts `file` back to `len`: before a retry, so what a failed append left is never followed by a second copy.
    fn truncate(&self, file: &File, path: &Path, len: u64) -> io::Result<()>;
    /// To the drive, without draining its cache (`durable::push`).
    fn push(&self, file: &File, path: &Path) -> io::Result<()>;
    /// Everything pushed so far, on the whole drive, lands before anything written after (`durable::order`).
    fn order(&self, file: &File, path: &Path) -> io::Result<()>;
    /// The drive's cache drained (`Durability::Full`): everything pushed is on the disk itself.
    fn barrier(&self, file: &File, path: &Path) -> io::Result<()>;
}

/// This OS's file steps.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsIo;

impl Io for OsIo {
    fn create(&self, path: &Path) -> io::Result<File> {
        if let Some(dir) = path.parent() {
            make_folders(dir)?;
        }
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(path)?;
        sync_parent(path)?;
        Ok(file)
    }

    fn open_append(&self, path: &Path) -> io::Result<File> {
        let file = crate::journal::open_append(path)?;
        sync_parent(path)?;
        Ok(file)
    }

    fn append(&self, file: &mut File, _path: &Path, bytes: &[u8]) -> io::Result<()> {
        file.write_all(bytes)
    }

    fn write_at(&self, file: &File, _path: &Path, at: u64, bytes: &[u8]) -> io::Result<()> {
        file.write_all_at(bytes, at)
    }

    fn truncate(&self, file: &File, _path: &Path, len: u64) -> io::Result<()> {
        file.set_len(len)
    }

    fn push(&self, file: &File, _path: &Path) -> io::Result<()> {
        durable::push(file)
    }

    fn order(&self, file: &File, _path: &Path) -> io::Result<()> {
        durable::order(file)
    }

    fn barrier(&self, file: &File, _path: &Path) -> io::Result<()> {
        durable::sync(file, Durability::Full)
    }
}

/// `durable::sync` through `io`: `Ordered` is the tick's push, then order (everything written to `file` lands before
/// anything written after); `Full` is the barrier (on the disk itself).
pub fn sync(io: &dyn Io, file: &File, path: &Path, durability: Durability) -> io::Result<()> {
    match durability {
        Durability::Ordered => {
            io.push(file, path)?;
            io.order(file, path)
        }
        Durability::Full => io.barrier(file, path),
    }
}

/// Makes `dir` and any folders above it that are missing, syncing each new one's entry into its parent.
fn make_folders(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        make_folders(parent)?;
    }
    match std::fs::create_dir(dir) {
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        other => other?,
    }
    sync_parent(dir)
}

/// Makes `path`'s entry in its folder durable (Linux; nothing elsewhere).
fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => durable::fsync_dir(dir),
        _ => Ok(()),
    }
}

#[cfg(feature = "synthetic")]
pub use logged::{Fault, IoOp, LoggedIo};

#[cfg(feature = "synthetic")]
mod logged {
    use std::fs::File;
    use std::io::{self, ErrorKind};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use super::{Io, OsIo};

    /// One step `LoggedIo` took.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum IoOp {
        Create(PathBuf),
        /// The file, the offset the bytes went to (its length before), and the bytes that reached it.
        Append(PathBuf, u64, Vec<u8>),
        Push(PathBuf),
        Barrier(PathBuf),
        OpenAppend(PathBuf),
        /// The file, the offset and the bytes written there (`Io::write_at`).
        Write(PathBuf, u64, Vec<u8>),
        /// Cut back to this length.
        Truncate(PathBuf, u64),
        Order(PathBuf),
    }

    /// What `LoggedIo` does wrong. `at_append` counts `Io::append` calls from 1, the head `open` writes included (an
    /// empty head writes nothing), retries included.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Fault {
        /// The disk fills at append `at_append`: `partial` bytes of it reach the disk, then it fails with ENOSPC, and so
        /// does every later append.
        Full { at_append: usize, partial: usize },
        /// Appends `at_append` to `at_append + times − 1` fail with an I/O error (EIO), each after the first half of its
        /// bytes reached the file (a short write, then the error); later ones succeed.
        Eio { at_append: usize, times: u32 },
        /// Every step waits this long first.
        Slow(Duration),
    }

    /// Does the real IO (into the test's temporary folder) and logs each step; with a `Fault`, fails as it says.
    #[derive(Debug, Default)]
    pub struct LoggedIo {
        fault: Option<Fault>,
        appends: AtomicUsize,
        ops: Mutex<Vec<IoOp>>,
    }

    impl LoggedIo {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_fault(f: Fault) -> Self {
            Self {
                fault: Some(f),
                ..Self::default()
            }
        }

        /// Every step so far, in order.
        pub fn ops(&self) -> Vec<IoOp> {
            self.ops
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn log(&self, op: IoOp) {
            self.ops
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(op);
        }

        fn wait(&self) {
            if let Some(Fault::Slow(d)) = self.fault {
                std::thread::sleep(d);
            }
        }
    }

    impl Io for LoggedIo {
        fn create(&self, path: &Path) -> io::Result<File> {
            self.wait();
            let file = OsIo.create(path)?;
            self.log(IoOp::Create(path.to_path_buf()));
            Ok(file)
        }

        fn open_append(&self, path: &Path) -> io::Result<File> {
            self.wait();
            let file = OsIo.open_append(path)?;
            self.log(IoOp::OpenAppend(path.to_path_buf()));
            Ok(file)
        }

        fn append(&self, file: &mut File, path: &Path, bytes: &[u8]) -> io::Result<()> {
            self.wait();
            let n = self.appends.fetch_add(1, Ordering::SeqCst) + 1;
            let at = file.metadata()?.len();
            let (landed, error) = match self.fault {
                Some(Fault::Full { at_append, partial }) if n >= at_append => {
                    let landed = if n == at_append {
                        partial.min(bytes.len())
                    } else {
                        0
                    };
                    let e = io::Error::new(
                        ErrorKind::StorageFull,
                        "no space left on device (injected)",
                    );
                    (landed, Some(e))
                }
                Some(Fault::Eio { at_append, times })
                    if n >= at_append && n - at_append < times as usize =>
                {
                    (
                        bytes.len() / 2,
                        Some(io::Error::other("input/output error (injected)")),
                    )
                }
                _ => (bytes.len(), None),
            };
            OsIo.append(file, path, &bytes[..landed])?;
            if landed > 0 {
                self.log(IoOp::Append(
                    path.to_path_buf(),
                    at,
                    bytes[..landed].to_vec(),
                ));
            }
            error.map_or(Ok(()), Err)
        }

        fn write_at(&self, file: &File, path: &Path, at: u64, bytes: &[u8]) -> io::Result<()> {
            self.wait();
            OsIo.write_at(file, path, at, bytes)?;
            self.log(IoOp::Write(path.to_path_buf(), at, bytes.to_vec()));
            Ok(())
        }

        fn truncate(&self, file: &File, path: &Path, len: u64) -> io::Result<()> {
            self.wait();
            OsIo.truncate(file, path, len)?;
            self.log(IoOp::Truncate(path.to_path_buf(), len));
            Ok(())
        }

        fn push(&self, file: &File, path: &Path) -> io::Result<()> {
            self.wait();
            OsIo.push(file, path)?;
            self.log(IoOp::Push(path.to_path_buf()));
            Ok(())
        }

        fn order(&self, file: &File, path: &Path) -> io::Result<()> {
            self.wait();
            OsIo.order(file, path)?;
            self.log(IoOp::Order(path.to_path_buf()));
            Ok(())
        }

        fn barrier(&self, file: &File, path: &Path) -> io::Result<()> {
            self.wait();
            OsIo.barrier(file, path)?;
            self.log(IoOp::Barrier(path.to_path_buf()));
            Ok(())
        }
    }
}
