//! A render worker's own folder: `<app data>/make/worker/<pid>-<n>/`, one per worker process, named after
//! the app's process so a later start knows a leftover. `in/` is written only by the parent (`profile.json`, then the
//! inputs of each request: project snapshots, contributions, SVGs, stills, the font list); `out/` and `tmp/` are the
//! worker's, the only folders its sandbox lets it write. The parent deletes the whole folder once the worker has
//! exited, and at the next start whatever an earlier run left, stopping a worker it left running first (`sweep`).
//! `in/` holds one request's inputs at a time (`clear_inputs`), and the parent measures what the worker
//! wrote (`written`).

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use klipolo_platform::process::sandbox::SandboxProfile;
use serde::{Deserialize, Serialize};

use super::protocol::{Input, InputName, PROFILE_NAME};

mod tree;

pub const IN: &str = "in";
pub const OUT: &str = "out";
pub const TMP: &str = "tmp";
/// The worker's process, in its session (which it can't write): `{pid, started, boot}`.
pub const WORKER_FILE: &str = "worker.json";

static NEXT: AtomicU32 = AtomicU32::new(1);

/// A worker's folder. The parent removes it with [`Session::remove`] once the worker is gone.
#[derive(Debug)]
pub struct Session {
    dir: PathBuf,
}

impl Session {
    /// A new, empty session under `root` (created if missing), with `in/`, `out/` and `tmp/`; its path is canonical
    /// (the sandbox matches resolved paths byte for byte).
    pub fn create(root: &Path) -> io::Result<Session> {
        std::fs::create_dir_all(root)?;
        let root = std::fs::canonicalize(root)?;
        let pid = std::process::id();
        for _ in 0..64 {
            let dir = root.join(format!("{pid}-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    for sub in [IN, OUT, TMP] {
                        std::fs::create_dir(dir.join(sub))?;
                    }
                    return Ok(Session { dir });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("no free session name"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `in/profile.json`: the paths the worker's sandbox may read, which it reads (and checks again) before anything.
    pub fn write_profile(&self, profile: &SandboxProfile) -> io::Result<()> {
        let json = serde_json::to_vec(profile).map_err(io::Error::other)?;
        self.replace_file(PROFILE_NAME, &json)
    }

    /// Writes `input` into `in/`, in place of an input of the same name. A file or folder appears whole: it is written
    /// under a hidden name first (no input name starts with `.`), then renamed.
    pub fn write_input(&self, input: &Input) -> io::Result<()> {
        match input {
            Input::File(name, bytes) => self.replace_file(name.as_str(), bytes),
            Input::Folder(name, files) => self.replace_folder(name, files),
        }
    }

    /// Deletes every input in `in/` (its profile stays): a request's inputs are gone once it is answered.
    pub fn clear_inputs(&self) -> io::Result<()> {
        for entry in std::fs::read_dir(self.dir.join(IN))? {
            let entry = entry?;
            if entry.file_name() == PROFILE_NAME {
                continue;
            }
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    /// What the worker wrote in `tmp/` and `out/`: bytes (files' lengths, so a sparse file counts whole) and entries,
    /// counting stops past `max_entries`. Links are counted, never followed; what can't be read makes it `unreadable`
    /// (fail closed).
    pub fn written(&self, max_entries: usize) -> Written {
        tree::walk(
            &[self.dir.join(TMP), self.dir.join(OUT)],
            max_entries,
            tree::list,
        )
    }

    /// Deletes the folder and everything in it, whatever the worker locked or nested there (`tree::remove_tree`). Only
    /// once its worker is gone (its watcher saw it exit) or never started: what was locked is unlocked.
    pub fn remove(&self) {
        if let Err(e) = tree::remove_tree(&self.dir, true)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(target: "make", "a worker session couldn't be deleted: {e}");
        }
    }

    fn replace_file(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let dir = self.dir.join(IN);
        let temp = dir.join(format!(".new-{name}"));
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, dir.join(name))
    }

    fn replace_folder(&self, name: &InputName, files: &[(InputName, Vec<u8>)]) -> io::Result<()> {
        let dir = self.dir.join(IN);
        let temp = dir.join(format!(".new-{}", name.as_str()));
        if temp.exists() {
            std::fs::remove_dir_all(&temp)?;
        }
        std::fs::create_dir(&temp)?;
        for (file, bytes) in files {
            std::fs::write(temp.join(file.as_str()), bytes)?;
        }
        let target = dir.join(name.as_str());
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&target)?,
            Ok(_) => std::fs::remove_file(&target)?,
            Err(_) => {}
        }
        std::fs::rename(&temp, &target)
    }
}

/// What a worker wrote into its session (`Session::written`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub bytes: u64,
    pub entries: usize,
    /// Something in there couldn't be read: over the cap whatever it holds.
    pub unreadable: bool,
    /// A folder more than 32 deep: over the cap too.
    pub too_deep: bool,
}

#[derive(Serialize, Deserialize)]
struct WorkerRecord {
    pid: u32,
    started: u64,
    boot: String,
}

/// Records the worker `pid` in `session` (`WORKER_FILE`: its id, start time and boot), so a later start can stop it if
/// the app never did. `Unsupported` where the OS won't say when a process started (Windows until W).
pub fn record_worker(session: &Path, pid: u32) -> io::Result<()> {
    let unsupported = || io::Error::from(io::ErrorKind::Unsupported);
    let record = WorkerRecord {
        pid,
        started: klipolo_platform::process::start_time(pid).ok_or_else(unsupported)?,
        boot: klipolo_platform::process::boot_id().ok_or_else(unsupported)?,
    };
    let temp = session.join(format!(".new-{WORKER_FILE}"));
    std::fs::write(
        &temp,
        serde_json::to_vec(&record).map_err(io::Error::other)?,
    )?;
    std::fs::rename(&temp, session.join(WORKER_FILE))
}

/// Whether the worker a leftover session records is gone: it wasn't running, or it was stopped now and that is
/// confirmed. False with no record, or when the kill can't be confirmed: then nothing in the session is unlocked.
fn stop_worker(session: &Path) -> bool {
    let mut text = Vec::new();
    let read = std::fs::File::open(session.join(WORKER_FILE))
        .and_then(|file| file.take(1024).read_to_end(&mut text));
    let Ok(Ok(record)) = read.map(|_| serde_json::from_slice::<WorkerRecord>(&text)) else {
        return false;
    };
    let running =
        || klipolo_platform::process::still_running(record.pid, record.started, &record.boot);
    if !running() {
        return true;
    }
    tracing::info!(target: "make", "stopping an effect checker an earlier run left");
    if let Err(e) = klipolo_platform::process::kill(record.pid, record.started) {
        tracing::warn!(target: "make", "a leftover effect checker couldn't be stopped: {e}");
        return false;
    }
    for _ in 0..40 {
        if !running() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

/// Deletes the sessions under `root` that no running worker's parent owns: a `<pid>-<n>` folder whose process is gone,
/// and anything else there that hasn't changed for 10 minutes; a worker such a session records that still runs is
/// stopped first, and what it locked is unlocked only once it is confirmed gone. This run's own are kept.
pub fn sweep(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|n| n.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        let modified = entry.metadata().and_then(|m| m.modified()).ok();
        if klipolo_platform::process::is_leftover(pid, modified) {
            let path = entry.path();
            let removed = if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let gone = stop_worker(&path);
                tree::remove_tree(&path, gone)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(e) = removed {
                tracing::warn!(target: "make", "a leftover worker session couldn't be deleted: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests;
