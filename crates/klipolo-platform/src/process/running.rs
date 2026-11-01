//! The processes running from the app's bundle: an update installs only once none is left but the
//! stateless `--mcp` bridge, so a running 1.4 never meets a 1.5 sidecar.

use std::io;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    pub pid: u32,
    /// Its mode argument (`argv[1]` when it starts with "--": "--mcp", "--export-worker"…); None for the app itself or
    /// when it can't be read.
    pub mode: Option<String>,
}

/// Processes other than this one whose executable lies inside `bundle`: workers, the recorder, chat servers, the MCP
/// bridge, another Klipolo. macOS `proc_listallpids` + `exe_path` + `KERN_PROCARGS2`; Linux `/proc/*/{exe,cmdline}`;
/// Windows later: Toolhelp (the stub answers an error for now, which is why Windows' `install::place` is Unknown).
///
/// A process whose arguments can't be read is listed with no mode, so it counts as running (the safe side). `Err` when
/// the process list itself can't be read (the Windows stub, a failed `proc_listallpids`, an unreadable `/proc`): an
/// unknown answer is never "nothing runs", so a caller treats it as something running.
pub fn running_from(bundle: &Path) -> io::Result<Vec<Running>> {
    // The OS reports each executable by its resolved path (macOS: /private/var, not /var).
    let bundle = std::fs::canonicalize(bundle).unwrap_or_else(|_| bundle.to_path_buf());
    crate::imp::running_from(&bundle)
}

/// Pure: the mode in an argument list (argv[1] when it starts with "--").
pub fn mode_of<S: AsRef<str>>(argv: &[S]) -> Option<String> {
    let arg = argv.get(1)?.as_ref();
    arg.starts_with("--").then(|| arg.to_owned())
}
