//! The recordings folder, measured once per recorder opening (the app runs it before `setup`): how long writing and
//! fully flushing 1 MB there takes (`setup`'s `write_ms`), and whether its file system can lock a file, as a take's
//! `session.lock` must (a folder that can't, as on some SMB shares, is refused up front with
//! `RecorderSetup::refuse_folder`; network drives aren't supported). Only a scratch file is written, and it is
//! removed again.

use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Write};
use std::path::Path;
use std::time::Instant;

use klipolo_platform::durable::{Durability, sync};
use klipolo_platform::process::lock_exclusive;

use super::RecorderSetup;
use crate::english::msg;

/// The scratch file's name under the recordings root.
const SCRATCH: &str = ".klipolo-folder-check";
const MEGABYTE: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderCheck {
    /// None when the folder couldn't be written (the take's own start then says why).
    pub write_ms: Option<u32>,
    /// False only when the file system refused the lock itself.
    pub locks: bool,
}

pub fn check_folder(root: &Path) -> FolderCheck {
    let path = root.join(SCRATCH);
    let checked = measure(root, &path);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != ErrorKind::NotFound
    {
        tracing::warn!(error = %e, "preflight: the folder check's scratch file stays");
    }
    checked.unwrap_or_else(|e| {
        tracing::warn!(root = %root.display(), error = %e, "preflight: the recordings folder can't be written");
        FolderCheck {
            write_ms: None,
            locks: true,
        }
    })
}

fn measure(root: &Path, path: &Path) -> io::Result<FolderCheck> {
    std::fs::create_dir_all(root)?;
    let started = Instant::now();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(&vec![0; MEGABYTE])?;
    sync(&file, Durability::Full)?;
    let write_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
    let locks = match lock_exclusive(&file) {
        Ok(()) => true,
        // Held elsewhere still means the file system locks.
        Err(e) if e.kind() == ErrorKind::WouldBlock => true,
        Err(e) => {
            tracing::warn!(root = %root.display(), error = %e, "preflight: the recordings folder can't lock a file");
            false
        }
    };
    Ok(FolderCheck {
        write_ms: Some(write_ms),
        locks,
    })
}

impl RecorderSetup {
    /// The recordings folder can't hold a take's lock (`FolderCheck::locks`: some SMB shares): refused up front with
    /// `capture.preflight.networkFolder`, among the drive's notes (network drives aren't supported).
    pub fn refuse_folder(&mut self) {
        let note = msg!("capture.preflight.networkFolder");
        if !self.notes.contains(&note) {
            let at = self
                .notes
                .iter()
                .position(|n| after_the_drive(&n.code))
                .unwrap_or(self.notes.len());
            self.notes.insert(at, note);
        }
    }
}

/// Whether a note comes after the drive's in `RecorderSetup::notes`: the mic's, the camera's and the battery's.
fn after_the_drive(code: &str) -> bool {
    matches!(
        code,
        "capture.preflight.virtualMic"
            | "capture.preflight.bluetoothMic"
            | "capture.preflight.micMissing"
            | "capture.permission.mic"
            | "capture.preflight.cameraBusy"
            | "capture.preflight.cameraOff"
            | "capture.permission.camera"
            | "capture.preflight.cameraWithScreen"
            | "capture.preflight.lowBattery"
    )
}
