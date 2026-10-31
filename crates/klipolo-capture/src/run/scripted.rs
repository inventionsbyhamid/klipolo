//! The session with the app's commands from a script (feature `synthetic`): the threaded tests and `synthetic-take`.

use std::io::Write;
use std::time::Duration;

use super::threads::run_session;
use crate::backend::Backend;
use crate::protocol::Command;
use crate::session::SessionDir;

/// `run_session` with the app's commands from a script: (ms after the start, command). Its end is the parent going.
pub fn run_with(
    backend: Box<dyn Backend>,
    dir: SessionDir,
    resume: bool,
    script: Vec<(u32, Command)>,
) -> i32 {
    let (reader, mut writer) = match std::io::pipe() {
        Ok(ends) => ends,
        Err(e) => {
            tracing::error!(error = %e, "run_with: no pipe");
            return 1;
        }
    };
    let started = std::time::Instant::now();
    std::thread::spawn(move || {
        for (ms, cmd) in script {
            let due = started + Duration::from_millis(u64::from(ms));
            std::thread::sleep(due.saturating_duration_since(std::time::Instant::now()));
            let mut line = serde_json::to_vec(&cmd).expect("a command serialises");
            line.push(b'\n');
            if writer.write_all(&line).is_err() {
                return;
            }
        }
    });
    run_session(
        backend,
        dir,
        resume,
        Box::new(std::io::BufReader::new(reader)),
        Box::new(std::io::sink()),
    )
}
