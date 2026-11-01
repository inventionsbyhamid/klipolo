//! Small files written whole through the writer (`Effect::WriteAtomic`): a cursor shape's PNG. Each is
//! a temporary file beside its place, synced, then renamed in (`durable::write_atomic`), on the writer's thread in order
//! with every append, so the PNG is on the disk before the `S` line that names it. A crash leaves it whole or absent. A
//! failure fails the writer like any other write: the take stops and saves.

use std::path::Path;

use klipolo_platform::durable::{self, Durability};

use super::thread::{Failed, Msg, Step, Thread};
use super::{DurableWriter, WriterError};

impl DurableWriter {
    /// Queues `bytes` to be written whole as `key` (its folder made if needed). Never blocks.
    pub fn write_atomic(&self, key: &str, bytes: Vec<u8>) -> Result<(), WriterError> {
        self.check()?;
        let path = self.dir.key_path(key).map_err(|e| {
            tracing::error!(key, error = %e, "writer: refused a key");
            WriterError::Gone
        })?;
        self.send(Msg::WriteAtomic { path, bytes })
    }
}

impl Thread {
    pub(super) fn write_whole(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failed> {
        let written = make_folder(path)
            .and_then(|()| durable::write_atomic(path, bytes, Durability::Ordered));
        match written {
            Ok(()) => {
                self.shared
                    .bytes
                    .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::AcqRel);
                Ok(())
            }
            Err(error) => Err(Failed {
                step: Step::Write,
                path: path.to_path_buf(),
                error,
            }),
        }
    }
}

/// `path`'s folder, made (and its entry synced) when it isn't there yet.
fn make_folder(path: &Path) -> std::io::Result<()> {
    let Some(dir) = path.parent().filter(|d| !d.is_dir()) else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    match dir.parent() {
        Some(up) if !up.as_os_str().is_empty() => durable::fsync_dir(up),
        _ => Ok(()),
    }
}
