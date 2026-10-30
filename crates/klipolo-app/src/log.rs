//! The app's log: stderr, and a file in the OS's logs folder (`~/Library/Logs/Klipolo/Klipolo.log` on macOS), started
//! afresh at each launch. `KLIPOLO_LOG=debug` (or `warn`, `trace`) changes how much it says; info by default. The log
//! names files, never their folders (the engine's own rule).

use std::fs::File;
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

/// The app's log: stderr and the log file (stderr alone when the file can't be made).
pub fn install() {
    let file = klipolo_platform::paths::logs_dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        File::create(dir.join("Klipolo.log")).ok()
    });
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
