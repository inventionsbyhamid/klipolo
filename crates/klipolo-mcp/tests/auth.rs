//! Who can reach Klipolo's MCP server: the connection file only this user can read, the token of each run, and the
//! HTTP server on 127.0.0.1 that refuses another program on the Mac (no token or a wrong one, a browser page, DNS
//! rebinding, a huge or slow body, a flood of connections or sessions) before it reads a body, and stays answering.

#![cfg(unix)]

#[path = "support/client.rs"]
mod client;
#[path = "support/fake_host.rs"]
mod fake_host;

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use client::{Client, SESSION_ID, head, hello, raw, read_raw, request};
use fake_host::FakeHost;
use klipolo_chat::{Job, Moment, Proposal, ToolCall};
use klipolo_mcp::connection::{
    self, ConnectionFile, FILE, McpServer, VERSION, new_token, support_dir_from,
};
use klipolo_mcp::host::{
    Applied, Host, HostError, OpenProposal, ProposalOutcome, Snapshot, Started,
};
use klipolo_mcp::rpc::{Mcp, listed};
use klipolo_mcp::server::{
    self, BODY_LIMIT, BODY_TIMEOUT, CONNECTIONS, EXPIRE_EVERY, HEAD_TIMEOUT, Running,
};
use klipolo_mcp::session::{IDLE, MAX_SESSIONS};
use klipolo_mcp::words::{
    NO_SUCH_SESSION, NOT_JSON, SESSION_FIRST, STOPPED, TOO_LARGE, TOO_SLOW, VERSION_UNKNOWN,
};
use klipolo_model::Transaction;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::runtime::Handle;

fn is_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[test]
fn new_token_is_64_hex_digits_and_new_each_time() {
    let (a, b) = (new_token(), new_token());
    assert!(is_token(&a), "{a}");
    assert!(is_token(&b), "{b}");
    assert_ne!(a, b);
}

#[test]
fn connection_file_is_private_and_reads_back() {
    let temp = tempfile::tempdir().unwrap();
    // The folder is made when needed.
    let dir = temp.path().join("Klipolo");
    let file = ConnectionFile {
        port: 51234,
        token: new_token(),
        pid: std::process::id(),
        version: VERSION,
    };
    let path = connection::write(&dir, &file).unwrap();
    assert_eq!(path, dir.join(FILE));
    assert_eq!(FILE, "mcp.json");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"port": 51234, "token": file.token, "pid": std::process::id(), "version": 1})
    );
    assert_eq!(connection::read(&dir), Some(file.clone()));
    // The token never shows in a log line.
    assert!(!format!("{file:?}").contains(&file.token));

    // A file left by a Klipolo that crashed is replaced.
    let again = ConnectionFile {
        port: 51235,
        token: new_token(),
        ..file.clone()
    };
    connection::write(&dir, &again).unwrap();
    assert_eq!(connection::read(&dir), Some(again));
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    // A file of another format, or none, reads as none.
    std::fs::remove_file(&path).unwrap();
    std::fs::write(
        &path,
        r#"{"port": 1, "token": "x", "pid": 2, "version": 2}"#,
    )
    .unwrap();
    assert_eq!(connection::read(&dir), None);
    std::fs::write(&path, "not json").unwrap();
    assert_eq!(connection::read(&dir), None);
    connection::remove(&dir);
    assert!(!path.exists());
    assert_eq!(connection::read(&dir), None);
    // Removing it again is fine.
    connection::remove(&dir);
}

#[test]
fn connection_file_is_never_written_through_a_link() {
    let temp = tempfile::tempdir().unwrap();
    let elsewhere = temp.path().join("elsewhere.json");
    std::fs::write(&elsewhere, "kept").unwrap();
    std::os::unix::fs::symlink(&elsewhere, temp.path().join(FILE)).unwrap();
    let file = ConnectionFile {
        port: 51234,
        token: new_token(),
        pid: 1,
        version: VERSION,
    };
    // The link goes, not what it points at; the new file is a file of its own.
    connection::write(temp.path(), &file).unwrap();
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "kept");
    let meta = std::fs::symlink_metadata(temp.path().join(FILE)).unwrap();
    assert!(meta.file_type().is_file());
}

/// The connection file is read only as a file of its own: a link to a good file, a folder or a FIFO at its name reads
/// as none, and a FIFO at once (a read that waited on it would hang the bridge, and the switch-off).
#[test]
fn connection_file_reads_only_as_a_file_of_its_own() {
    let temp = tempfile::tempdir().unwrap();
    let elsewhere = temp.path().join("elsewhere");
    let file = ConnectionFile {
        port: 51234,
        token: new_token(),
        pid: std::process::id(),
        version: VERSION,
    };
    let good = connection::write(&elsewhere, &file).unwrap();
    assert_eq!(connection::read(&elsewhere), Some(file));
    let dir = temp.path().join("Klipolo");
    std::fs::create_dir(&dir).unwrap();
    std::os::unix::fs::symlink(&good, dir.join(FILE)).unwrap();
    assert_eq!(connection::read(&dir), None, "a link");
    std::fs::remove_file(dir.join(FILE)).unwrap();
    std::fs::create_dir(dir.join(FILE)).unwrap();
    assert_eq!(connection::read(&dir), None, "a folder");
    std::fs::remove_dir(dir.join(FILE)).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(dir.join(FILE))
        .status()
        .unwrap();
    assert!(made.success());
    // On a thread, so a read that waits fails the test instead of hanging it.
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = said.send(connection::read(&dir));
    });
    assert_eq!(
        heard.recv_timeout(Duration::from_secs(2)),
        Ok(None),
        "a FIFO, at once"
    );
}

/// A connection file others may read is none of Klipolo's: `write` makes it 0600, so one in Klipolo's format that
/// anyone else could read (or write) was put there by someone else, and the bridge never sends an app's calls to it.
#[test]
fn a_connection_file_others_may_read_reads_as_none() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let file = ConnectionFile {
        port: 51234,
        token: new_token(),
        pid: std::process::id(),
        version: VERSION,
    };
    let path = connection::write(temp.path(), &file).unwrap();
    assert_eq!(connection::read(temp.path()), Some(file));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(connection::read(temp.path()), None);
}

#[test]
fn support_dir_is_the_variable_else_application_support() {
    let os = |s: &str| Some(OsString::from(s));
    assert_eq!(
        support_dir_from(os("/tmp/klipolo-test"), os("/Users/someone")),
        Some(PathBuf::from("/tmp/klipolo-test"))
    );
    assert_eq!(
        support_dir_from(None, os("/Users/someone")),
        Some(PathBuf::from(
            "/Users/someone/Library/Application Support/Klipolo"
        ))
    );
    // Set but empty counts as unset.
    assert_eq!(
        support_dir_from(os(""), os("/Users/someone")),
        Some(PathBuf::from(
            "/Users/someone/Library/Application Support/Klipolo"
        ))
    );
    assert_eq!(support_dir_from(None, None), None);
    assert_eq!(connection::SUPPORT_DIR_VAR, "KLIPOLO_SUPPORT_DIR");
}

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A server on the test's runtime over the two-clip project, taking `TOKEN`.
fn serve() -> (Running, Arc<FakeHost>) {
    let host = FakeHost::two_clips();
    let mcp = Arc::new(Mcp::new(host.clone()));
    let running = server::serve(&Handle::current(), mcp, TOKEN.to_owned()).unwrap();
    (running, host)
}

/// `Host: 127.0.0.1:{port}`.
fn ours(port: u16) -> (&'static str, String) {
    ("Host", format!("127.0.0.1:{port}"))
}

fn bearer(token: &str) -> (&'static str, String) {
    ("Authorization", format!("Bearer {token}"))
}

/// A POST head to `/mcp` that claims a body of `length` bytes, with `headers` after the type and length.
fn post_head(length: usize, headers: &[(&'static str, String)]) -> String {
    let mut all = vec![
        ("Content-Type", "application/json".to_owned()),
        ("Content-Length", length.to_string()),
    ];
    all.extend_from_slice(headers);
    head("POST", "/mcp", &all)
}

/// A whole `initialize` request as the app `name`, the connection closed after it.
fn initialize_bytes(headers: &[(&'static str, String)], name: &str) -> String {
    let body = request(1, "initialize", hello(name)).to_string();
    let mut all = headers.to_vec();
    all.push(("Connection", "close".to_owned()));
    post_head(body.len(), &all) + &body
}

/// Refused from its head alone, within a second, and the connection closed with the answer.
fn assert_refused_closed(answer: &client::Raw, status: u16, what: &str) {
    assert_eq!(answer.status, status, "{what}: {answer:?}");
    assert_eq!(answer.header("connection"), Some("close"), "{what}");
    assert!(answer.closed, "{what}: the connection closes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_or_wrong_token_is_401_before_the_body_is_read() {
    let (server, host) = serve();
    let port = server.port;
    let mut checker = Client::new(port, TOKEN);
    checker.initialize("Checker").await;
    let snapshots = host.state().counts.snapshots;
    let wrong: Vec<Option<String>> = vec![
        None,
        Some("Bearer wrong".into()),
        Some(format!("Bearer {}", &TOKEN[..63])),
        Some(format!("Bearer {TOKEN}0")),
        Some(format!("Bearer {}", TOKEN.to_uppercase())),
        Some(format!("Basic {TOKEN}")),
        Some(TOKEN.into()),
        Some("Bearer ".into()),
    ];
    for auth in wrong {
        let mut headers = vec![ours(port)];
        if let Some(auth) = &auth {
            headers.push(("Authorization", auth.clone()));
        }
        // The head claims a megabyte, and no body follows: the answer comes from the head alone.
        let started = Instant::now();
        let bytes = post_head(1 << 20, &headers);
        let answer = raw(port, bytes.as_bytes(), Duration::from_secs(1)).await;
        assert_refused_closed(&answer, 401, &format!("{auth:?}"));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(answer.body, "");
    }
    // A client library with an empty token hears the same.
    let stranger = Client::new(port, "");
    let answer = stranger
        .post(&request(1, "initialize", hello("Stranger")))
        .await;
    assert_eq!(answer.status(), 401);
    // Nothing reached Klipolo: no session opened, nothing read, and it still answers.
    assert_eq!(server.connected(), ["Checker"]);
    assert_eq!(host.state().counts.snapshots, snapshots);
    assert!(checker.answers().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_origin_is_403() {
    let (server, _host) = serve();
    let port = server.port;
    let mut checker = Client::new(port, TOKEN);
    checker.initialize("Checker").await;
    // Any page a browser shows, even one served from Klipolo's own address, and with the token.
    for origin in [
        "http://evil.test".to_owned(),
        "null".to_owned(),
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        "chrome-extension://abcdef".to_owned(),
    ] {
        for token in [Some(TOKEN), None] {
            let mut headers = vec![ours(port), ("Origin", origin.clone())];
            headers.extend(token.map(bearer));
            let bytes = post_head(1 << 20, &headers);
            let answer = raw(port, bytes.as_bytes(), Duration::from_secs(1)).await;
            assert_refused_closed(&answer, 403, &origin);
        }
    }
    assert_eq!(server.connected(), ["Checker"]);
    assert!(checker.answers().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_host_is_403() {
    let (server, _host) = serve();
    let port = server.port;
    let mut checker = Client::new(port, TOKEN);
    checker.initialize("Checker").await;
    // DNS rebinding: a page's own name pointed at 127.0.0.1. Also another port, no port, no `Host`, or two.
    let hosts: Vec<Vec<(&str, String)>> = vec![
        vec![("Host", "evil.test".into())],
        vec![("Host", format!("evil.test:{port}"))],
        vec![("Host", format!("localhost.evil.test:{port}"))],
        vec![("Host", format!("127.0.0.1:{}", port.wrapping_add(1)))],
        vec![("Host", "127.0.0.1".into())],
        vec![("Host", format!("[::1]:{port}"))],
        vec![],
        vec![ours(port), ("Host", "evil.test".into())],
    ];
    for host in hosts {
        let mut headers = host.clone();
        headers.push(bearer(TOKEN));
        let bytes = post_head(1 << 20, &headers);
        let answer = raw(port, bytes.as_bytes(), Duration::from_secs(1)).await;
        assert_refused_closed(&answer, 403, &format!("{host:?}"));
    }
    // Its own names are served: 127.0.0.1 and localhost, in any case.
    for name in ["127.0.0.1", "localhost", "LocalHost"] {
        let headers = [("Host", format!("{name}:{port}")), bearer(TOKEN)];
        let bytes = initialize_bytes(&headers, "Agent");
        let answer = raw(port, bytes.as_bytes(), Duration::from_secs(2)).await;
        assert_eq!(answer.status, 200, "{name}: {answer:?}");
    }
    assert!(checker.answers().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listens_on_loopback_only() {
    let (server, _host) = serve();
    let port = server.port;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_ok());
    // Not on IPv6's loopback.
    assert!(TcpStream::connect(("::1", port)).await.is_err());
    // Not on this machine's address on its network, when it has one.
    let outward = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("192.0.2.1:9")?;
            socket.local_addr()
        })
        .ok()
        .map(|addr| addr.ip())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified());
    if let Some(ip) = outward {
        let reached =
            tokio::time::timeout(Duration::from_secs(1), TcpStream::connect((ip, port))).await;
        assert!(!matches!(reached, Ok(Ok(_))), "{ip}:{port} answered");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_token_each_start() {
    let temp = tempfile::tempdir().unwrap();
    let mcp = Arc::new(Mcp::new(FakeHost::two_clips()));
    let first = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path()).unwrap();
    let a = connection::read(temp.path()).expect("a connection file");
    assert!(is_token(&a.token), "{}", a.token);
    assert_eq!(a.port, first.port());
    // The file's token is the one the server takes.
    let mut agent = Client::new(a.port, &a.token);
    agent.initialize("Agent").await;
    first.stop();

    let second = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path()).unwrap();
    let b = connection::read(temp.path()).expect("a connection file");
    assert!(is_token(&b.token), "{}", b.token);
    assert_ne!(a.token, b.token);
    // The last run's token is refused.
    let stale = Client::new(b.port, &a.token);
    let answer = stale.post(&request(1, "initialize", hello("Agent"))).await;
    assert_eq!(answer.status(), 401);
    let mut agent = Client::new(b.port, &b.token);
    agent.initialize("Agent").await;
    second.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_file_is_private_and_goes_on_stop() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("Klipolo");
    let mcp = Arc::new(Mcp::new(FakeHost::two_clips()));
    let server = McpServer::start(&Handle::current(), mcp, &dir).unwrap();
    let path = dir.join(FILE);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let file = connection::read(&dir).expect("a connection file");
    assert_eq!(file.pid, std::process::id());
    assert_eq!(file.port, server.port());
    assert_eq!(file.version, 1);
    let port = server.port();
    let mut agent = Client::new(port, &file.token);
    agent.initialize("Agent").await;
    assert_eq!(server.connected(), ["Agent"]);
    server.stop();
    assert!(!path.exists());
    // The port closes.
    let mut closed = false;
    for _ in 0..40 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(closed, "still listening on {port}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_another_server_wrote_stays_on_stop() {
    let temp = tempfile::tempdir().unwrap();
    let mcp = Arc::new(Mcp::new(FakeHost::two_clips()));
    let server = McpServer::start(&Handle::current(), mcp, temp.path()).unwrap();
    let other = ConnectionFile {
        port: 1,
        token: new_token(),
        pid: 1,
        version: VERSION,
    };
    connection::write(temp.path(), &other).unwrap();
    server.stop();
    assert_eq!(connection::read(temp.path()), Some(other));
}

/// Another Klipolo on the Mac serving the support folder's file (a dev build beside the installed app): a second
/// server is refused, the file stays the first's byte for byte, so the apps keep reaching it, and the first's stop
/// still deletes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_leaves_a_live_servers_file() {
    let temp = tempfile::tempdir().unwrap();
    let mcp = Arc::new(Mcp::new(FakeHost::two_clips()));
    let first = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path()).unwrap();
    let path = temp.path().join(FILE);
    let written = std::fs::read(&path).unwrap();
    let file = connection::read(temp.path()).expect("a connection file");
    assert_eq!(connection::served(temp.path()), Some(file.clone()));

    let second = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path());
    let refused = second.err().expect("a second server is refused");
    assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&path).unwrap(), written);
    let mut agent = Client::new(file.port, &file.token);
    agent.initialize("Agent").await;
    first.stop();
    assert!(!path.exists());
}

/// A file no server answers for is no one's, and the next start replaces it: its process is gone (a crash), or alive
/// but its port is closed, or answers to another token (the process id and the port went to other programs).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_replaces_a_file_no_server_answers_for() {
    let temp = tempfile::tempdir().unwrap();
    let mcp = Arc::new(Mcp::new(FakeHost::two_clips()));
    let other = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path()).unwrap();
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let dead = gone.id();
    gone.wait().unwrap();
    for (pid, port) in [(dead, other.port()), (1, 9), (1, other.port())] {
        let left = ConnectionFile {
            port,
            token: new_token(),
            pid,
            version: VERSION,
        };
        connection::write(temp.path(), &left).unwrap();
        assert_eq!(connection::served(temp.path()), None, "{left:?}");
        let server = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path())
            .unwrap_or_else(|e| panic!("{left:?}: {e}"));
        let file = connection::read(temp.path()).expect("a connection file");
        assert_eq!((file.pid, file.port), (std::process::id(), server.port()));
        assert_ne!(file.token, left.token);
        server.stop();
        assert_eq!(connection::read(temp.path()), None);
    }
    other.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_body_is_413() {
    let (server, host) = serve();
    let port = server.port;
    let mut checker = Client::new(port, TOKEN);
    checker.initialize("Checker").await;
    assert_eq!(BODY_LIMIT, 256 * 1024);
    // A length past the limit is refused from the head, before a byte of the body is read.
    let bytes = post_head(BODY_LIMIT + 1, &[ours(port), bearer(TOKEN)]);
    let answer = raw(port, bytes.as_bytes(), Duration::from_secs(1)).await;
    assert_refused_closed(&answer, 413, "a length past the limit");
    assert_eq!(answer.json()["error"]["message"], TOO_LARGE);
    // A body sent in chunks, with no length, is refused once it passes the limit.
    let headers = [
        ours(port),
        bearer(TOKEN),
        ("Content-Type", "application/json".to_owned()),
        ("Transfer-Encoding", "chunked".to_owned()),
    ];
    let mut bytes = head("POST", "/mcp", &headers).into_bytes();
    bytes.extend_from_slice(format!("{:x}\r\n", BODY_LIMIT + 1).as_bytes());
    bytes.extend(std::iter::repeat_n(b' ', BODY_LIMIT + 1));
    let answer = raw(port, &bytes, Duration::from_secs(2)).await;
    assert_refused_closed(&answer, 413, "chunks past the limit");
    // A body of exactly the limit is read.
    let mut body = request(1, "initialize", hello("Padded"))
        .to_string()
        .into_bytes();
    body.resize(BODY_LIMIT, b' ');
    let answer = checker.post_bytes(body).await;
    assert_eq!(answer.status(), 200);
    assert_eq!(
        host.state().connected.last().unwrap(),
        &["Checker", "Padded"]
    );
    assert!(checker.answers().await);
}

#[tokio::test(start_paused = true)]
async fn slow_body_is_408_after_10_s() {
    let (server, _host) = serve();
    let port = server.port;
    assert_eq!(BODY_TIMEOUT, Duration::from_secs(10));
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let bytes = post_head(100, &[ours(port), bearer(TOKEN)]) + "{\"jsonrpc\"";
    stream.write_all(bytes.as_bytes()).await.unwrap();
    let started = tokio::time::Instant::now();
    let early = read_raw(&mut stream, BODY_TIMEOUT - Duration::from_millis(100)).await;
    assert_eq!((early.status, early.closed), (0, false), "{early:?}");
    // On the paused test clock a timer this far off can fire a few seconds late (the clock jumps from timer to timer
    // while the runtime waits on sockets): the refusal is checked to come after the timeout and well before twice it.
    let answer = read_raw(&mut stream, BODY_TIMEOUT).await;
    let waited = started.elapsed();
    assert_refused_closed(&answer, 408, "a slow body");
    assert_eq!(answer.json()["error"]["message"], TOO_SLOW);
    assert!(
        waited >= BODY_TIMEOUT && waited < 2 * BODY_TIMEOUT,
        "{waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn slow_head_is_closed_after_10_s() {
    let (server, _host) = serve();
    let port = server.port;
    assert_eq!(HEAD_TIMEOUT, Duration::from_secs(10));
    // A head that never ends, and a connection that sends nothing.
    let started_head = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    for sent in [started_head.as_str(), ""] {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(sent.as_bytes()).await.unwrap();
        let started = tokio::time::Instant::now();
        let early = read_raw(&mut stream, HEAD_TIMEOUT - Duration::from_millis(100)).await;
        assert_eq!(
            (early.status, early.closed),
            (0, false),
            "{sent:?}: {early:?}"
        );
        // As for a slow body: after the timeout, and well before twice it, on the paused test clock.
        let answer = read_raw(&mut stream, HEAD_TIMEOUT).await;
        let waited = started.elapsed();
        assert!(answer.closed, "{sent:?}: open after {waited:?}");
        assert!(!(200..300).contains(&answer.status), "{answer:?}");
        assert!(
            waited >= HEAD_TIMEOUT && waited < 2 * HEAD_TIMEOUT,
            "{waited:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_past_16_wait_until_one_closes() {
    let (server, _host) = serve();
    let port = server.port;
    assert_eq!(CONNECTIONS, 16);
    let mut held = Vec::new();
    for _ in 0..CONNECTIONS {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(b"POST /mcp HTTP/1.1\r\n").await.unwrap();
        held.push(stream);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    // One more waits, unread, while the others are open.
    let mut waiting = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let bytes = initialize_bytes(&[ours(port), bearer(TOKEN)], "Agent");
    waiting.write_all(bytes.as_bytes()).await.unwrap();
    let early = read_raw(&mut waiting, Duration::from_millis(500)).await;
    assert_eq!((early.status, early.closed), (0, false), "{early:?}");
    assert!(server.connected().is_empty());
    // One closes: the one waiting is served.
    drop(held.pop());
    let answer = read_raw(&mut waiting, Duration::from_secs(2)).await;
    assert_eq!(answer.status, 200, "{answer:?}");
    assert_eq!(server.connected(), ["Agent"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_is_405() {
    let (server, _host) = serve();
    let port = server.port;
    // Klipolo sends nothing unasked, so it offers no stream.
    let get = head("GET", "/mcp", &[ours(port), bearer(TOKEN)]);
    let answer = raw(port, get.as_bytes(), Duration::from_secs(1)).await;
    assert_refused_closed(&answer, 405, "GET");
    assert!(answer.header("allow").unwrap().contains("POST"));
    let put = post_head(0, &[ours(port), bearer(TOKEN)]).replacen("POST", "PUT", 1);
    let answer = raw(port, put.as_bytes(), Duration::from_secs(1)).await;
    assert_refused_closed(&answer, 405, "PUT");
    // Without the token, the token is what's refused.
    let get = head("GET", "/mcp", &[ours(port)]);
    let answer = raw(port, get.as_bytes(), Duration::from_secs(1)).await;
    assert_refused_closed(&answer, 401, "GET without the token");
    // Any other path is nowhere.
    for path in [
        "/",
        "/mcp/",
        "/mcp/identity",
        "/.well-known/oauth-protected-resource",
    ] {
        let bytes = post_head(0, &[ours(port), bearer(TOKEN)]).replacen("/mcp", path, 1);
        let answer = raw(port, bytes.as_bytes(), Duration::from_secs(1)).await;
        assert_refused_closed(&answer, 404, path);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_json_is_32700() {
    let (server, _host) = serve();
    let client = Client::new(server.port, TOKEN);
    for body in [&b"{not json"[..], b"", b"\xff\xfe"] {
        let answer = client.post_bytes(body.to_vec()).await;
        assert_eq!(answer.status(), 400);
        let reply: Value = answer.json().await.unwrap();
        assert_eq!(
            reply,
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": NOT_JSON}})
        );
    }
    // JSON that isn't one message is the protocol's to refuse.
    let answer = client.post(&json!([request(1, "ping", json!({}))])).await;
    assert_eq!(answer.status(), 200);
    let reply: Value = answer.json().await.unwrap();
    assert_eq!(reply["error"]["code"], -32600);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_opens_a_session_and_the_rest_need_it() {
    let (server, _host) = serve();
    let mut client = Client::new(server.port, TOKEN);
    // Before a session: only `initialize`.
    let answer = client.post(&request(5, "ping", json!({}))).await;
    assert_eq!(answer.status(), 400);
    let reply: Value = answer.json().await.unwrap();
    assert_eq!(
        reply,
        json!({"jsonrpc": "2.0", "id": 5, "error": {"code": -32600, "message": SESSION_FIRST}})
    );
    let answer = client.post(&request(1, "initialize", hello("Agent"))).await;
    assert_eq!(answer.status(), 200);
    assert_eq!(answer.headers()["content-type"], "application/json");
    let session = answer.headers()[client::SESSION_ID]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        session.len() == 32 && session.bytes().all(|b| b.is_ascii_hexdigit()),
        "{session}"
    );
    let reply: Value = answer.json().await.unwrap();
    assert_eq!(reply["result"]["serverInfo"]["name"], "klipolo");
    client.session = Some(session);
    // A notification is accepted with nothing to answer.
    let ready = client
        .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    assert_eq!(ready.status(), 202);
    assert_eq!(ready.bytes().await.unwrap().len(), 0);
    assert_eq!(client.ask("ping", json!({})).await["result"], json!({}));
    let tools = client.ask("tools/list", json!({})).await;
    assert_eq!(tools["result"], listed("tools/list").unwrap());
    // A session Klipolo doesn't know: the client opens a new one.
    client.session = Some("0".repeat(32));
    let answer = client.post(&request(6, "ping", json!({}))).await;
    assert_eq!(answer.status(), 404);
    let reply: Value = answer.json().await.unwrap();
    assert_eq!(
        reply,
        json!({"jsonrpc": "2.0", "id": 6, "error": {"code": -32001, "message": NO_SUCH_SESSION}})
    );
}

/// The two-clip host, each project read taking `takes`; how many reads ran, and how many at once at most.
struct Slow {
    host: Arc<FakeHost>,
    takes: Duration,
    reads: AtomicUsize,
    now: AtomicUsize,
    most: AtomicUsize,
}

const READ_TAKES: Duration = Duration::from_millis(300);

impl Slow {
    fn new(takes: Duration) -> Arc<Slow> {
        Arc::new(Slow {
            host: FakeHost::two_clips(),
            takes,
            reads: AtomicUsize::new(0),
            now: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
        })
    }
}

impl Host for Slow {
    fn snapshot(&self) -> Option<Snapshot> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.takes);
        self.now.fetch_sub(1, Ordering::SeqCst);
        self.host.snapshot()
    }
    fn open_proposal(&self) -> Option<OpenProposal> {
        self.host.open_proposal()
    }
    fn apply(&self, client: &str, base: u64, tx: &Transaction) -> Result<Applied, HostError> {
        self.host.apply(client, base, tx)
    }
    fn propose(
        &self,
        client: &str,
        base: u64,
        calls: &[ToolCall],
        proposal: Proposal,
        replacing: Option<&str>,
    ) -> Result<String, HostError> {
        self.host.propose(client, base, calls, proposal, replacing)
    }
    fn outcome(&self, client: &str) -> ProposalOutcome {
        self.host.outcome(client)
    }
    fn undo_own(&self, client: &str) -> Result<Applied, HostError> {
        self.host.undo_own(client)
    }
    fn show_moments(&self, client: &str, moments: &[Moment]) {
        self.host.show_moments(client, moments)
    }
    fn estimate_usd(&self, job: &Job) -> f64 {
        self.host.estimate_usd(job)
    }
    fn start_paid(&self, client: &str, job: &Job) -> Result<Started, HostError> {
        self.host.start_paid(client, job)
    }
    fn connected(&self, clients: &[String]) {
        self.host.connected(clients)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn messages_are_handled_one_at_a_time_and_refusals_never_wait() {
    let slow = Slow::new(READ_TAKES);
    let mcp = Arc::new(Mcp::new(slow.clone()));
    let server = server::serve(&Handle::current(), mcp, TOKEN.to_owned()).unwrap();
    let port = server.port;
    let mut apps = Vec::new();
    for name in ["One", "Two", "Three"] {
        let mut app = Client::new(port, TOKEN);
        app.initialize(name).await;
        apps.push(app);
    }
    // Three apps read the project at once: the reads run one after another, on threads of their own.
    let reads = apps.into_iter().map(|app| {
        tokio::spawn(async move {
            let context = app.call("getContext", json!({})).await;
            assert_eq!(context["isError"], false, "{context}");
        })
    });
    let reads: Vec<_> = reads.collect();
    tokio::time::sleep(READ_TAKES / 3).await;
    // Meanwhile the server answers at once what it refuses from the head.
    let started = Instant::now();
    let bytes = post_head(1 << 20, &[ours(port)]);
    let answer = raw(port, bytes.as_bytes(), Duration::from_secs(2)).await;
    assert_refused_closed(&answer, 401, "while reads run");
    assert!(started.elapsed() < READ_TAKES, "{:?}", started.elapsed());
    for read in reads {
        read.await.unwrap();
    }
    assert_eq!(slow.most.load(Ordering::SeqCst), 1);
}

/// A request after `initialize` whose `MCP-Protocol-Version` Klipolo doesn't speak is 400, and nothing is handled; one
/// without the header is taken at the version agreed. `initialize` agrees on the version in its body, so a header
/// there is never refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_protocol_version_header_is_400() {
    let (server, host) = serve();
    let port = server.port;
    let mut client = Client::new(port, TOKEN);
    client.initialize("Agent").await;
    let session = client.session.clone().unwrap();
    let send = |method: &'static str, version: Option<&'static str>| {
        let session = session.clone();
        async move {
            let body = request(7, method, json!({})).to_string();
            let mut headers = vec![
                ours(port),
                bearer(TOKEN),
                (SESSION_ID, session),
                ("Connection", "close".to_owned()),
            ];
            headers.extend(version.map(|v| ("MCP-Protocol-Version", v.to_owned())));
            let bytes = post_head(body.len(), &headers) + &body;
            raw(port, bytes.as_bytes(), Duration::from_secs(2)).await
        }
    };
    let snapshots = host.state().counts.snapshots;
    for version in ["2099-01-01", "2026-07-28", "2024-11-05", ""] {
        let answer = send("tools/list", Some(version)).await;
        assert_eq!(answer.status, 400, "{version}: {answer:?}");
        assert_eq!(
            answer.json(),
            json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32600, "message": VERSION_UNKNOWN}}),
            "{version}"
        );
    }
    assert_eq!(host.state().counts.snapshots, snapshots, "nothing handled");
    for version in [Some("2025-06-18"), Some("2025-11-25"), None] {
        let answer = send("ping", version).await;
        assert_eq!(answer.status, 200, "{version:?}: {answer:?}");
        assert_eq!(answer.json()["result"], json!({}));
    }
    // initialize with a version Klipolo doesn't speak in the header: agreed in the body, at Klipolo's latest.
    let body = request(1, "initialize", hello("Agent")).to_string();
    let headers = [
        ours(port),
        bearer(TOKEN),
        ("MCP-Protocol-Version", "2026-07-28".to_owned()),
        ("Connection", "close".to_owned()),
    ];
    let bytes = post_head(body.len(), &headers) + &body;
    let answer = raw(port, bytes.as_bytes(), Duration::from_secs(2)).await;
    assert_eq!(answer.status, 200, "{answer:?}");
    // DELETE with a version Klipolo doesn't speak ends nothing.
    let bytes = head(
        "DELETE",
        "/mcp",
        &[
            ours(port),
            bearer(TOKEN),
            (SESSION_ID, client.session.clone().unwrap()),
            ("MCP-Protocol-Version", "2099-01-01".to_owned()),
            ("Connection", "close".to_owned()),
        ],
    );
    let answer = raw(port, bytes.as_bytes(), Duration::from_secs(2)).await;
    assert_eq!(answer.status, 400, "{answer:?}");
    assert!(client.answers().await, "the session is still open");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_session_is_404_and_delete_ends_one() {
    let (server, host) = serve();
    let port = server.port;
    let mut agent = Client::new(port, TOKEN);
    agent.initialize("Agent").await;
    let mut other = Client::new(port, TOKEN);
    other.initialize("Other app").await;
    assert_eq!(server.connected(), ["Agent", "Other app"]);
    // The app ends its session: it is gone, and the app no longer shows as connected.
    assert_eq!(agent.end().await, 204);
    assert_eq!(server.connected(), ["Other app"]);
    assert_eq!(host.state().connected.last().unwrap(), &["Other app"]);
    let answer = agent.post(&request(4, "ping", json!({}))).await;
    assert_eq!(answer.status(), 404);
    let reply: Value = answer.json().await.unwrap();
    assert_eq!(reply["error"]["message"], NO_SUCH_SESSION);
    // Ending it again, or one Klipolo never knew, is 404; without a session id, 400.
    assert_eq!(agent.end().await, 404);
    agent.session = Some("f".repeat(32));
    assert_eq!(agent.end().await, 404);
    agent.session = None;
    assert_eq!(agent.end().await, 400);
    // Ending a session needs the token, like the rest.
    let session = other.session.clone().unwrap();
    let delete = head("DELETE", "/mcp", &[ours(port), (SESSION_ID, session)]);
    let answer = raw(port, delete.as_bytes(), Duration::from_secs(1)).await;
    assert_refused_closed(&answer, 401, "DELETE without the token");
    assert!(other.answers().await);
    assert_eq!(server.connected(), ["Other app"]);
}

/// A clock a test moves by hand, from `start`.
fn hand_clock(
    start: Instant,
) -> (
    Arc<Mutex<Instant>>,
    impl Fn() -> Instant + Send + Sync + 'static,
) {
    let clock = Arc::new(Mutex::new(start));
    let read = Arc::clone(&clock);
    (clock, move || *read.lock().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flood_of_sessions_is_paced_and_the_app_connected_stays() {
    let host = FakeHost::two_clips();
    let start = Instant::now();
    let (clock, now) = hand_clock(start);
    let set = |secs: u64| *clock.lock().unwrap() = start + Duration::from_secs(secs);
    let mcp = Arc::new(Mcp::with_clock(host.clone(), now));
    let server = server::serve(&Handle::current(), mcp, TOKEN.to_owned()).unwrap();
    let port = server.port;
    let mut live = Client::new(port, TOKEN);
    live.initialize("Live app").await;
    assert_eq!(live.call("getContext", json!({})).await["isError"], false);

    // Without the token, a flood opens nothing: each is refused from its head, and its connection closes.
    for k in 0..40 {
        let bytes = initialize_bytes(&[ours(port)], &format!("Stranger {k}"));
        let answer = raw(port, bytes.as_bytes(), Duration::from_secs(1)).await;
        assert_refused_closed(&answer, 401, "a session without the token");
    }
    assert_eq!(server.connected(), ["Live app"]);

    // With it, one name past 10 sessions a minute waits, and is told how long.
    set(1);
    for _ in 0..10 {
        Client::new(port, TOKEN).initialize("Flood").await;
    }
    let answer = (Client::new(port, TOKEN))
        .post(&request(1, "initialize", hello("Flood")))
        .await;
    assert_eq!(answer.status(), 429);
    assert_eq!(answer.headers()["retry-after"], "60");
    assert!(answer.headers().get(SESSION_ID).is_none());
    let reply: Value = answer.json().await.unwrap();
    assert_eq!(
        reply,
        json!({"jsonrpc": "2.0", "id": 1, "error": {
            "code": -32000,
            "message": "Klipolo is pacing new sessions. Wait 1 minute, then try again.",
        }})
    );

    // Many names, minute after minute: all apps together open 20 a minute, and the app connected, working now and
    // then, keeps its session while the least recently used go.
    let mut opened_each_minute = Vec::new();
    for minute in 0..5u64 {
        set(60 * minute + 30);
        let summary = live.call("getSummary", json!({})).await;
        assert_eq!(summary["isError"], false, "{summary}");
        set(60 * minute + 31);
        let mut opened = 0;
        for k in 0..25 {
            let name = format!("Flood {minute}.{k}");
            let answer = (Client::new(port, TOKEN))
                .post(&request(1, "initialize", hello(&name)))
                .await;
            match answer.status().as_u16() {
                200 => opened += 1,
                429 => {
                    let wait: u64 = answer.headers()["retry-after"]
                        .to_str()
                        .unwrap()
                        .parse()
                        .unwrap();
                    assert!((1..=60).contains(&wait), "{wait}");
                }
                other => panic!("{other}"),
            }
        }
        opened_each_minute.push(opened);
        let connected = server.connected();
        assert!(connected.len() <= MAX_SESSIONS, "{}", connected.len());
        assert!(
            connected.contains(&"Live app".to_owned()),
            "minute {minute}"
        );
    }
    // The first minute had room for 9 more: the live app's session and the 10 of one name came first.
    assert_eq!(opened_each_minute, [9, 20, 20, 20, 20]);
    assert!(live.answers().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_ends_sessions_and_starting_again_keeps_the_paces() {
    let temp = tempfile::tempdir().unwrap();
    let host = FakeHost::two_clips();
    let mcp = Arc::new(Mcp::new(host.clone()));
    let first = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path()).unwrap();
    let file = connection::read(temp.path()).unwrap();
    let mut agent = Client::new(file.port, &file.token);
    for _ in 0..10 {
        agent.initialize("Agent").await;
    }
    // A connection kept open after its request.
    let mut idle = TcpStream::connect(("127.0.0.1", file.port)).await.unwrap();
    let body = request(1, "initialize", hello("Checker")).to_string();
    let bytes = post_head(body.len(), &[ours(file.port), bearer(&file.token)]) + &body;
    idle.write_all(bytes.as_bytes()).await.unwrap();
    let answer = read_raw(&mut idle, Duration::from_millis(500)).await;
    assert_eq!((answer.status, answer.closed), (200, false), "{answer:?}");
    assert_eq!(first.connected(), ["Agent", "Checker"]);

    first.stop();
    // The open connection closes, and no app shows as connected.
    let after = read_raw(&mut idle, Duration::from_secs(3)).await;
    assert!(after.closed, "{after:?}");
    assert!(mcp.connected().is_empty());
    assert_eq!(
        host.state().connected.last().unwrap(),
        &Vec::<String>::new()
    );

    // Turned on again: a new token, the last run's sessions gone, and the same paces. The app opened 10 sessions this
    // minute, so its 11th waits; another app is welcome.
    let second = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path()).unwrap();
    let file = connection::read(temp.path()).unwrap();
    let agent = Client::new(file.port, &file.token);
    let answer = agent.post(&request(1, "initialize", hello("Agent"))).await;
    assert_eq!(answer.status(), 429);
    let mut other = Client::new(file.port, &file.token);
    other.initialize("Other app").await;
    assert_eq!(second.connected(), ["Other app"]);
    second.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_of_the_last_run_is_404() {
    let temp = tempfile::tempdir().unwrap();
    let mcp = Arc::new(Mcp::new(FakeHost::two_clips()));
    let first = McpServer::start(&Handle::current(), Arc::clone(&mcp), temp.path()).unwrap();
    let file = connection::read(temp.path()).unwrap();
    let mut agent = Client::new(file.port, &file.token);
    agent.initialize("Agent").await;
    first.stop();
    let second = McpServer::start(&Handle::current(), mcp, temp.path()).unwrap();
    let file = connection::read(temp.path()).unwrap();
    let mut again = Client::new(file.port, &file.token);
    again.session = agent.session.clone();
    let answer = again.post(&request(2, "ping", json!({}))).await;
    assert_eq!(answer.status(), 404);
    // It opens a new one.
    again.initialize("Agent").await;
    assert!(again.answers().await);
    second.stop();
}

#[tokio::test(start_paused = true)]
async fn idle_sessions_go_every_minute() {
    let host = FakeHost::two_clips();
    let start = Instant::now();
    let (clock, now) = hand_clock(start);
    let mcp = Arc::new(Mcp::with_clock(host.clone(), now));
    let server = server::serve(&Handle::current(), mcp, TOKEN.to_owned()).unwrap();
    assert_eq!(EXPIRE_EVERY, Duration::from_secs(60));
    let mut agent = Client::new(server.port, TOKEN);
    agent.initialize("Agent").await;
    // Unused for longer than `IDLE`: it goes when the server next looks, within a minute or so.
    *clock.lock().unwrap() = start + IDLE + Duration::from_secs(1);
    assert_eq!(server.connected(), ["Agent"]);
    let began = tokio::time::Instant::now();
    while !server.connected().is_empty() && began.elapsed() < 3 * EXPIRE_EVERY {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(server.connected().is_empty(), "after {:?}", began.elapsed());
    assert_eq!(
        host.state().connected.last().unwrap(),
        &Vec::<String>::new()
    );
    let answer = agent.post(&request(2, "ping", json!({}))).await;
    assert_eq!(answer.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_handled_once_stopped() {
    let slow = Slow::new(Duration::from_secs(1));
    let mcp = Arc::new(Mcp::new(slow.clone()));
    let server = server::serve(&Handle::current(), Arc::clone(&mcp), TOKEN.to_owned()).unwrap();
    let port = server.port;
    let mut first = Client::new(port, TOKEN);
    first.initialize("One").await;
    let mut second = Client::new(port, TOKEN);
    second.initialize("Two").await;
    // One app's read runs, and another's waits its turn.
    let running = tokio::spawn(async move { first.call("getContext", json!({})).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let waiting = tokio::spawn(async move {
        let read = json!({"name": "getContext", "arguments": {}});
        second.post(&request(2, "tools/call", read)).await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The person turns the server off: the read under way finishes, and the one waiting is never handled.
    server.stop();
    let read = running.await.unwrap();
    assert_eq!(read["isError"], false, "{read}");
    let answer = waiting.await.unwrap();
    assert_eq!(answer.status(), 503);
    let reply: Value = answer.json().await.unwrap();
    assert_eq!(reply["error"]["message"], STOPPED);
    assert_eq!(slow.reads.load(Ordering::SeqCst), 1);
    assert!(mcp.connected().is_empty());
}
