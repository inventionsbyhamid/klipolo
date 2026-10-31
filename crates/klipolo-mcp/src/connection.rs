//! The connection file, `<support>/mcp.json`: the port the server listens on and the token it takes, so the stdio
//! bridge, and any app the person points at it, can reach Klipolo. Only this user can read it (mode 0600). It is
//! written once the server listens, and deleted when the server stops (`McpServer`). Another Klipolo on the Mac (a dev
//! build beside the installed app) shares the support folder: a server starts only while no other serves the file
//! (`served`), and deletes it only while it is its own, so the apps keep reaching the Klipolo that lets them in.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

use crate::rpc::Mcp;
use crate::server::{self, PATH, Running};

/// The file's name in the support folder.
pub const FILE: &str = "mcp.json";
/// The file's format.
pub const VERSION: u32 = 1;
/// Sets the support folder in place of `~/Library/Application Support/Klipolo` (tests, and a second install).
pub const SUPPORT_DIR_VAR: &str = "KLIPOLO_SUPPORT_DIR";
/// The most of the file read: it is a few dozen bytes.
const MOST: u64 = 4096;
/// How long asking whether a server answers on the file's port may take to connect, and then to hear back.
const ASK_TIMEOUT: Duration = Duration::from_secs(1);
/// How Klipolo's server answers `GET /mcp` with its token: it offers no stream (a wrong token is 401).
const ANSWER: &[u8; 12] = b"HTTP/1.1 405";

/// What the file holds: `{"port": 51234, "token": "…", "pid": 4321, "version": 1}`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionFile {
    /// The port on 127.0.0.1.
    pub port: u16,
    /// The bearer token of this run: 64 hex digits.
    pub token: String,
    /// The process of the Klipolo that wrote it.
    pub pid: u32,
    pub version: u32,
}

/// Never prints the token.
impl fmt::Debug for ConnectionFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionFile")
            .field("port", &self.port)
            .field("token", &"…")
            .field("pid", &self.pid)
            .field("version", &self.version)
            .finish()
    }
}

/// Klipolo's support folder: `$KLIPOLO_SUPPORT_DIR` when set, else `$HOME/Library/Application Support/Klipolo` (the
/// folder this Mac's id is kept in). None without either.
pub fn support_dir() -> Option<PathBuf> {
    support_dir_from(std::env::var_os(SUPPORT_DIR_VAR), std::env::var_os("HOME"))
}

/// `support_dir`, from the values of `KLIPOLO_SUPPORT_DIR` and `HOME`; an empty value counts as unset.
pub fn support_dir_from(set: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let given = |v: Option<OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    given(set).or_else(|| {
        given(home).map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("Klipolo")
        })
    })
}

/// A token as Klipolo makes them: 64 lowercase hex digits.
pub fn is_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A new bearer token: 64 hex digits from two random `u128`s.
pub fn new_token() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}

/// Writes `f` to `<dir>/mcp.json`, readable only by this user, making `dir` when needed; where it went. Whatever was
/// there goes first (a file left by a Klipolo that crashed); the new file is made afresh and never through a link
/// (`klipolo_platform::file::write_private`), so one that appears there meanwhile fails the write.
pub fn write(dir: &Path, f: &ConnectionFile) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let body = serde_json::to_vec(f).map_err(io::Error::other)?;
    klipolo_platform::file::write_private(&path, &body)?;
    Ok(path)
}

/// The file in `dir`, when there is one Klipolo wrote in this format. It is read only as a file of its own
/// (`klipolo_platform::file::read_private`): a link, a folder, or a FIFO that would never answer, put at its name,
/// reads as none at once, and so does a file another user owns or others may read (Klipolo writes it 0600).
pub fn read(dir: &Path) -> Option<ConnectionFile> {
    let bytes = klipolo_platform::file::read_private(&dir.join(FILE), MOST).ok()?;
    serde_json::from_slice::<ConnectionFile>(&bytes)
        .ok()
        .filter(|f| f.version == VERSION)
}

/// The file in `dir` while a server serves it: the process that wrote it is alive, and its port answers to its token
/// as Klipolo's server does. A file a crash left is no one's, also once its process id or its port went to another
/// program. Asking takes a moment only while the process lives: at most `ASK_TIMEOUT` to connect, and as long again
/// for the answer.
pub fn served(dir: &Path) -> Option<ConnectionFile> {
    read(dir).filter(|f| klipolo_platform::process::is_alive(f.pid) && answers(f))
}

/// Whether Klipolo's server answers on `f.port` to `f.token`: `GET /mcp` with it is 405, as `server` answers it once
/// the token is taken. Only to 127.0.0.1, and only a token as Klipolo makes them.
fn answers(f: &ConnectionFile) -> bool {
    if !is_token(&f.token) {
        return false;
    }
    let to = SocketAddr::from((Ipv4Addr::LOCALHOST, f.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&to, ASK_TIMEOUT) else {
        return false;
    };
    let ask = format!(
        "GET {PATH} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        f.port, f.token
    );
    let mut head = [0; ANSWER.len()];
    stream.set_read_timeout(Some(ASK_TIMEOUT)).is_ok()
        && stream.set_write_timeout(Some(ASK_TIMEOUT)).is_ok()
        && stream.write_all(ask.as_bytes()).is_ok()
        && stream.read_exact(&mut head).is_ok()
        && head == *ANSWER
}

/// Deletes `<dir>/mcp.json`; none there is fine.
pub fn remove(dir: &Path) {
    match std::fs::remove_file(dir.join(FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            tracing::warn!(error = %e, "the MCP connection file couldn't be deleted");
        }
        _ => {}
    }
}

/// The server while it runs, with its connection file. Dropping it stops it, as `stop` does.
pub struct McpServer {
    running: Running,
    dir: PathBuf,
    token: String,
}

impl McpServer {
    /// Listens on 127.0.0.1 with a new token, serving `mcp` on `rt`, then writes `<dir>/mcp.json`. A file that can't
    /// be written stops the server again. The app keeps one `Mcp` for its whole run and starts it here each time the
    /// person turns the server on, so the paces, the allowance for jobs and an app's waiting proposal carry over.
    /// Refused (`AlreadyExists`) while another server serves the file (`served`): another Klipolo on the Mac lets apps
    /// in, and keeps its file.
    pub fn start(rt: &Handle, mcp: Arc<Mcp>, dir: &Path) -> io::Result<McpServer> {
        if served(dir).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "another Klipolo serves the MCP connection file",
            ));
        }
        let token = new_token();
        let running = server::serve(rt, mcp, token.clone())?;
        let file = ConnectionFile {
            port: running.port,
            token: token.clone(),
            pid: std::process::id(),
            version: VERSION,
        };
        write(dir, &file)?;
        Ok(McpServer {
            running,
            dir: dir.to_owned(),
            token,
        })
    }

    /// Stops the server (`server::Running::stop`) and deletes the connection file, while it is still this server's
    /// (this process's, with this run's token).
    pub fn stop(self) {
        drop(self);
    }

    pub fn port(&self) -> u16 {
        self.running.port
    }

    /// The apps connected now, each once, sorted.
    pub fn connected(&self) -> Vec<String> {
        self.running.connected()
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if read(&self.dir).is_some_and(|f| f.pid == std::process::id() && f.token == self.token) {
            remove(&self.dir);
        }
    }
}
