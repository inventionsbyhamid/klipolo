//! The app's log: stderr, and a file in the OS's logs folder (`~/Library/Logs/Klipolo/Klipolo.log` on macOS), started
//! afresh at each launch. The launch before keeps its log as `Klipolo.1.log`, so what happened before Klipolo opened
//! again (Reopen Klipolo, a crash) can still be read. `KLIPOLO_LOG=debug` (or `warn`, `trace`) changes how much it
//! says; info by default. The log names files, never their folders (the engine's own rule).

use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

use tracing::Level;
use tracing_subscriber::fmt::writer::MakeWriterExt;

fn level() -> Level {
    match std::env::var("KLIPOLO_LOG").as_deref() {
        Ok("trace") => Level::TRACE,
        Ok("debug") => Level::DEBUG,
        Ok("warn") => Level::WARN,
        Ok("error") => Level::ERROR,
        _ => Level::INFO,
    }
}

/// This launch's log file.
const LOG_FILE: &str = "Klipolo.log";
/// The launch before's.
const PREVIOUS_LOG_FILE: &str = "Klipolo.1.log";

/// This launch's log file in `dir`, made afresh; the launch before's is kept as `Klipolo.1.log`, in place of the one
/// before that. None when the file can't be made.
fn open_log(dir: &Path) -> Option<File> {
    std::fs::create_dir_all(dir).ok()?;
    let log = dir.join(LOG_FILE);
    match std::fs::rename(&log, dir.join(PREVIOUS_LOG_FILE)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!("the last launch's log couldn't be kept: {error}"),
    }
    File::create(log).ok()
}

/// The app's log: stderr and the log file (stderr alone when the file can't be made).
pub fn install() {
    let file = klipolo_platform::paths::logs_dir().and_then(|dir| open_log(&dir));
    let builder = tracing_subscriber::fmt().with_max_level(level());
    let installed = match file {
        Some(file) => builder
            .with_writer(std::io::stderr.and(Mutex::new(file)))
            .try_init(),
        None => builder.with_writer(std::io::stderr).try_init(),
    };
    if let Err(error) = installed {
        eprintln!("the log couldn't start: {error}");
    }
}

/// A worker's log: stderr, which the app reads.
pub fn install_worker() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(level())
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_keeps_the_last_launchs_log_as_klipolo_1() {
        let dir = tempfile::tempdir().unwrap();
        let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
        // The first launch: no log before it.
        drop(open_log(dir.path()).unwrap());
        assert!(!dir.path().join(PREVIOUS_LOG_FILE).exists());
        std::fs::write(dir.path().join(LOG_FILE), "before Reopen Klipolo").unwrap();
        // Klipolo opens again: its log starts empty, and the one before is kept.
        drop(open_log(dir.path()).unwrap());
        assert_eq!(read(LOG_FILE), "");
        assert_eq!(read(PREVIOUS_LOG_FILE), "before Reopen Klipolo");
        // Once more: only the launch before is kept.
        std::fs::write(dir.path().join(LOG_FILE), "after it").unwrap();
        drop(open_log(dir.path()).unwrap());
        assert_eq!(read(PREVIOUS_LOG_FILE), "after it");
        assert_eq!(read(LOG_FILE), "");
    }
}
