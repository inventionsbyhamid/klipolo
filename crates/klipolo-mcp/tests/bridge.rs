//! The stdio bridge (`klipolo-app --mcp`): what it answers by itself while Klipolo is off, how it reaches Klipolo
//! through the connection file (read again for each message, a dead process counting as off), the session it opens
//! under the client's name and opens again, what it does with each status, and that it writes nothing but JSON lines.
//! Klipolo is either the real server over the two-clip project, or a program on a port that records what it got.

#![cfg(unix)]

#[path = "support/fake_host.rs"]
mod fake_host;

use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use fake_host::FakeHost;
use klipolo_mcp::bridge::{Bridge, IN_FLIGHT, LINE_LIMIT, REPLY_LIMIT};
use klipolo_mcp::connection::{self, ConnectionFile, McpServer, VERSION};
use klipolo_mcp::rpc::{Mcp, initialize_result, listed, tool_result};
use klipolo_mcp::words::{
    BUSY, MAY_BE_MADE, NO_METHOD, NOT_JSON, OFF, ONE_MESSAGE, REPLY_TOO_LARGE, TOO_LARGE,
    UNANSWERED, sessions_paced,
};
use serde_json::{Value, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// What the bridge wrote to its stdout.
#[derive(Clone, Default)]
struct Out(Arc<Mutex<Vec<u8>>>);

impl Write for Out {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Out {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).expect("UTF-8")
    }

    /// Each line, as JSON: a line that isn't fails the test.
    fn lines(&self) -> Vec<Value> {
        (self.text().lines())
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
            .collect()
    }

    /// The one reply to request `id`.
    fn reply(&self, id: i64) -> Value {
        let replies: Vec<Value> = (self.lines().into_iter())
            .filter(|line| line["id"] == json!(id))
            .collect();
        assert_eq!(replies.len(), 1, "one reply to {id}: {replies:?}");
        replies[0].clone()
    }

    /// The text of tool call `id`'s result, which must be an error.
    fn refused(&self, id: i64) -> String {
        let reply = self.reply(id);
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

/// A bridge to Klipolo's support folder `dir`, and what it writes.
fn bridge(dir: &Path) -> (Bridge, Out) {
    let out = Out::default();
    let bridge = Bridge::new(dir.to_owned(), Box::new(out.clone())).expect("a bridge");
    (bridge, out)
}

fn request(id: i64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// The client's `initialize`, as the app `name`.
fn hello(id: i64, name: &str) -> String {
    let params = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": name, "version": "1.0"},
    });
    request(id, "initialize", params)
}

/// A call of tool `name`.
fn call(id: i64, name: &str, arguments: Value) -> String {
    request(
        id,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
}

/// The connection file in `dir`, naming `port`, `token` and `pid`.
fn file_for(dir: &Path, port: u16, token: &str, pid: u32) {
    let file = ConnectionFile {
        port,
        token: token.to_owned(),
        pid,
        version: VERSION,
    };
    connection::write(dir, &file).unwrap();
}

/// Klipolo's server on a runtime of its own over the two-clip project, its connection file in a folder of its own.
struct Klipolo {
    server: Option<McpServer>,
    mcp: Arc<Mcp>,
    host: Arc<FakeHost>,
    dir: tempfile::TempDir,
    rt: tokio::runtime::Runtime,
}

impl Klipolo {
    fn start() -> Klipolo {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let host = FakeHost::two_clips();
        let mcp = Arc::new(Mcp::new(host.clone()));
        let dir = tempfile::tempdir().unwrap();
        let server = McpServer::start(rt.handle(), Arc::clone(&mcp), dir.path()).unwrap();
        Klipolo {
            server: Some(server),
            mcp,
            host,
            dir,
            rt,
        }
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// The person turns access off and on: a new port and token, the same `Mcp`.
    fn restart(&mut self) {
        drop(self.server.take());
        let mcp = Arc::clone(&self.mcp);
        self.server = Some(McpServer::start(self.rt.handle(), mcp, self.dir.path()).unwrap());
    }

    fn connected(&self) -> Vec<String> {
        self.mcp.connected()
    }
}

/// One request a program got.
#[derive(Clone, Debug)]
struct Got {
    /// `POST`, `DELETE`.
    verb: String,
    /// Each header, its name in lower case.
    headers: Vec<(String, String)>,
    body: String,
}

impl Got {
    fn header(&self, name: &str) -> Option<&str> {
        (self.headers.iter())
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    /// The JSON-RPC method, or the HTTP verb when there is none.
    fn method(&self) -> String {
        match self.json()["method"].as_str() {
            Some(method) => method.to_owned(),
            None => self.verb.clone(),
        }
    }
}

/// A program on a free port of 127.0.0.1 that records each request and answers it with `answer`'s raw HTTP.
struct Program {
    port: u16,
    got: Arc<Mutex<Vec<Got>>>,
}

impl Program {
    fn got(&self) -> Vec<Got> {
        self.got.lock().unwrap().clone()
    }

    fn methods(&self) -> Vec<String> {
        self.got().iter().map(Got::method).collect()
    }
}

fn program(answer: impl Fn(&Got) -> Vec<u8> + Send + Sync + 'static) -> Program {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let got = Arc::new(Mutex::new(Vec::new()));
    let (seen, answer) = (Arc::clone(&got), Arc::new(answer));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (seen, answer) = (Arc::clone(&seen), Arc::clone(&answer));
            std::thread::spawn(move || {
                let Some(got) = read_request(&stream) else {
                    return;
                };
                seen.lock().unwrap().push(got.clone());
                let _ = (&stream).write_all(&answer(&got));
            });
        }
    });
    Program { port, got }
}

fn read_request(stream: &TcpStream) -> Option<Got> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let verb = line.split(' ').next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    let length = (headers.iter())
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    let body = String::from_utf8_lossy(&body).into_owned();
    Some(Got {
        verb,
        headers,
        body,
    })
}

/// A whole HTTP/1.1 answer, the connection closed after it.
fn http(status: u16, headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {status} Answer\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    (head + "\r\n" + body).into_bytes()
}

fn json_http(status: u16, body: &Value) -> Vec<u8> {
    http(
        status,
        &[("Content-Type", "application/json")],
        &body.to_string(),
    )
}

/// As Klipolo answers: `initialize` opens session `s-1` at 2025-06-18, a notification is 202, DELETE is 204, and a
/// request's result says "From Klipolo.".
fn like_klipolo(got: &Got) -> Vec<u8> {
    let msg = got.json();
    if got.verb == "DELETE" {
        return http(204, &[], "");
    }
    match msg["method"].as_str() {
        Some("initialize") => http(
            200,
            &[
                ("Content-Type", "application/json"),
                ("Mcp-Session-Id", "s-1"),
            ],
            &json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"protocolVersion": "2025-06-18"}})
                .to_string(),
        ),
        _ if msg.get("id").is_none() => http(202, &[], ""),
        _ => json_http(
            200,
            &json!({"jsonrpc": "2.0", "id": msg["id"], "result": tool_result("From Klipolo.", false)}),
        ),
    }
}

fn from_klipolo(out: &Out, id: i64) {
    let reply = out.reply(id);
    assert_eq!(
        reply,
        json!({"jsonrpc": "2.0", "id": id, "result": tool_result("From Klipolo.", false)})
    );
}

#[test]
fn answers_initialize_and_lists_tools_while_klipolo_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let (bridge, out) = bridge(dir.path());
    bridge.line(&hello(1, "Example Client"));
    bridge.line(&request(2, "tools/list", json!({})));
    bridge.line(&request(3, "resources/list", json!({})));
    bridge.line(&request(4, "resources/templates/list", json!({})));
    bridge.line(&request(5, "ping", json!({})));
    // A version Klipolo doesn't speak: its latest.
    let newer = json!({"protocolVersion": "2099-01-01", "clientInfo": {"name": "Example Client"}});
    bridge.line(&request(6, "initialize", newer));
    bridge.wait();
    let answered = |id: i64, result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
    assert_eq!(
        out.lines(),
        vec![
            answered(1, initialize_result("2025-06-18")),
            answered(2, listed("tools/list").unwrap()),
            answered(3, listed("resources/list").unwrap()),
            answered(4, listed("resources/templates/list").unwrap()),
            answered(5, json!({})),
            answered(6, initialize_result("2099-01-01")),
        ]
    );
    let lines = out.lines();
    assert_eq!(lines[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "klipolo");
    assert_eq!(lines[5]["result"]["protocolVersion"], "2025-11-25");
    // Every tool, the edit tools and the reads.
    let tools = lines[1]["result"]["tools"].as_array().unwrap();
    for name in ["getContext", "getProposal", "setFormat", "cutRange"] {
        assert!(tools.iter().any(|t| t["name"] == name), "{name}");
    }
}

#[test]
fn forwards_a_call_with_token_and_session() {
    let k = Klipolo::start();
    let (bridge, out) = bridge(k.dir());
    bridge.line(&hello(1, "Example Client"));
    bridge.wait();
    // The session opens as soon as the client says hello, under the client's own name.
    assert_eq!(k.connected(), ["Example Client"]);
    bridge.line(&call(2, "getContext", json!({})));
    bridge.wait();
    let before = k.host.revision();
    let format = json!({"format": "square", "keepOthers": true});
    bridge.line(&call(3, "setFormat", format));
    bridge.line(&request(
        4,
        "resources/read",
        json!({"uri": "klipolo://context"}),
    ));
    bridge.line(&request(5, "no/such/method", json!({})));
    bridge.wait();
    let context = out.reply(2)["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(context.starts_with("Revision "), "{context}");
    let applied = out.reply(3);
    assert_eq!(applied["result"]["isError"], false, "{applied}");
    assert_eq!(applied["result"]["structuredContent"]["outcome"], "done");
    assert_eq!(k.host.revision(), before + 1);
    assert_eq!(k.host.state().applied.last().unwrap().0, "Example Client");
    let read = out.reply(4);
    assert!(
        read["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Revision "),
        "{read}"
    );
    assert_eq!(out.reply(5)["error"]["code"], -32601);
    assert!(!out.text().contains(TOKEN));
    // One session for all of it, which ends when the client's input does.
    assert_eq!(k.host.state().connected, vec![vec!["Example Client"]]);
    bridge.serve(Cursor::new(Vec::new()));
    assert!(k.connected().is_empty());
    assert_eq!(out.lines().len(), 5);
}

/// What goes with each request: the token, the session and the version Klipolo agreed to (none of either with
/// `initialize`), the client's own `initialize` params, and never an `Origin`.
#[test]
fn sends_the_token_session_and_version_and_never_an_origin() {
    let klipolo = program(like_klipolo);
    let dir = tempfile::tempdir().unwrap();
    file_for(dir.path(), klipolo.port, TOKEN, std::process::id());
    let (bridge, out) = bridge(dir.path());
    bridge.line(&hello(1, "Muse Spark agent"));
    bridge.wait();
    bridge.line(&call(2, "getSummary", json!({})));
    bridge.serve(Cursor::new(Vec::new()));
    from_klipolo(&out, 2);
    let got = klipolo.got();
    assert_eq!(
        klipolo.methods(),
        [
            "initialize",
            "notifications/initialized",
            "tools/call",
            "DELETE"
        ]
    );
    let sent: Value = serde_json::from_str(&hello(1, "Muse Spark agent")).unwrap();
    assert_eq!(got[0].json()["params"], sent["params"]);
    assert_eq!(
        got[2].json(),
        serde_json::from_str::<Value>(&call(2, "getSummary", json!({}))).unwrap()
    );
    for (n, request) in got.iter().enumerate() {
        let bearer = format!("Bearer {TOKEN}");
        assert_eq!(
            request.header("authorization"),
            Some(bearer.as_str()),
            "{n}"
        );
        let host = format!("127.0.0.1:{}", klipolo.port);
        assert_eq!(request.header("host"), Some(host.as_str()), "{n}");
        assert_eq!(request.header("origin"), None, "{n}");
        if n == 0 {
            assert_eq!(request.header("mcp-session-id"), None);
            assert_eq!(request.header("mcp-protocol-version"), None);
        } else {
            assert_eq!(request.header("mcp-session-id"), Some("s-1"), "{n}");
            assert_eq!(
                request.header("mcp-protocol-version"),
                Some("2025-06-18"),
                "{n}"
            );
        }
        if request.verb == "POST" {
            assert_eq!(request.header("content-type"), Some("application/json"));
            let accept = request.header("accept").unwrap_or_default();
            assert!(accept.contains("application/json") && accept.contains("text/event-stream"));
        }
    }
}

#[test]
fn rereads_the_file_after_a_restart() {
    let mut k = Klipolo::start();
    let (bridge, out) = bridge(k.dir());
    bridge.line(&hello(1, "Muse Spark agent"));
    bridge.line(&call(2, "getSummary", json!({})));
    bridge.wait();
    assert_eq!(out.reply(2)["result"]["isError"], false);
    let first = connection::read(k.dir()).unwrap();
    k.restart();
    let second = connection::read(k.dir()).unwrap();
    assert_ne!(first.token, second.token);
    assert!(k.connected().is_empty());
    bridge.line(&call(3, "getSummary", json!({})));
    bridge.wait();
    let reply = out.reply(3);
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert_eq!(k.connected(), ["Muse Spark agent"]);
}

#[test]
fn opens_a_new_session_after_404() {
    let k = Klipolo::start();
    let (bridge, out) = bridge(k.dir());
    bridge.line(&hello(1, "Agent"));
    bridge.line(&call(2, "getSummary", json!({})));
    bridge.wait();
    // Klipolo forgets the session (it went idle, say): the next call opens a new one under the same name.
    k.mcp.end_all();
    assert!(k.connected().is_empty());
    bridge.line(&call(3, "getSummary", json!({})));
    bridge.wait();
    let reply = out.reply(3);
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert_eq!(k.connected(), ["Agent"]);

    // A session that is never known: one new session, then the call hears it got no answer.
    let forgetful = program(|got| match got.method().as_str() {
        "tools/call" => json_http(
            404,
            &json!({"jsonrpc": "2.0", "id": got.json()["id"], "error": {"code": -32001, "message": "No."}}),
        ),
        _ => like_klipolo(got),
    });
    let dir = tempfile::tempdir().unwrap();
    file_for(dir.path(), forgetful.port, TOKEN, std::process::id());
    let (bridge, out) = self::bridge(dir.path());
    bridge.line(&call(4, "getSummary", json!({})));
    bridge.wait();
    assert_eq!(out.refused(4), UNANSWERED);
    assert_eq!(
        forgetful.methods(),
        [
            "initialize",
            "notifications/initialized",
            "tools/call",
            "initialize",
            "notifications/initialized",
            "tools/call",
        ]
    );
}

#[test]
fn dead_pid_counts_as_off() {
    let klipolo = program(like_klipolo);
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    // A process that ended, and ids that name no one process (0 is the caller's group, past i32 every process).
    for pid in [dead, 0, i32::MAX as u32 + 1, u32::MAX] {
        let dir = tempfile::tempdir().unwrap();
        file_for(dir.path(), klipolo.port, TOKEN, pid);
        let (bridge, out) = bridge(dir.path());
        bridge.line(&hello(1, "Agent"));
        bridge.line(&call(2, "getSummary", json!({})));
        bridge.wait();
        assert_eq!(out.refused(2), OFF, "{pid}");
    }
    assert!(klipolo.got().is_empty(), "the token never went to the port");
    // A process that is alive counts, even one of another user's (EPERM).
    let dir = tempfile::tempdir().unwrap();
    file_for(dir.path(), klipolo.port, TOKEN, 1);
    let (bridge, out) = bridge(dir.path());
    bridge.line(&call(2, "getSummary", json!({})));
    bridge.wait();
    from_klipolo(&out, 2);
}

#[test]
fn a_call_while_off_says_how_to_turn_it_on() {
    assert_eq!(
        OFF,
        "Klipolo isn't running, or \"Let other apps edit with Klipolo\" is off. Ask the person to open Klipolo and \
turn it on in Settings."
    );
    assert_eq!(UNANSWERED, "Klipolo stopped before it answered. Try again.");
    let dir = tempfile::tempdir().unwrap();
    let (bridge, out) = bridge(dir.path());
    let pid = std::process::id();
    let mut asked = 0;
    let mut off = |what: &str| {
        asked += 2;
        let cut = json!({"startSec": 1.0, "endSec": 2.0});
        bridge.line(&call(asked, "cutRange", cut));
        bridge.line(&request(
            asked + 1,
            "resources/read",
            json!({"uri": "klipolo://context"}),
        ));
        bridge.wait();
        assert_eq!(
            out.reply(asked),
            json!({"jsonrpc": "2.0", "id": asked, "result": tool_result(OFF, true)}),
            "{what}"
        );
        assert_eq!(
            out.reply(asked + 1),
            json!({"jsonrpc": "2.0", "id": asked + 1, "error": {"code": -32000, "message": OFF}}),
            "{what}"
        );
    };
    off("no connection file");
    // A file left behind, its port no longer listening.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    file_for(dir.path(), port, TOKEN, pid);
    off("nothing on the port");
    // A token that isn't one Klipolo makes, or a file of another format: not Klipolo's, and nothing is sent.
    let klipolo = program(like_klipolo);
    file_for(dir.path(), klipolo.port, "secret", pid);
    off("not a token");
    std::fs::remove_file(dir.path().join(connection::FILE)).unwrap();
    let other = json!({"port": klipolo.port, "token": TOKEN, "pid": pid, "version": 2});
    std::fs::write(dir.path().join(connection::FILE), other.to_string()).unwrap();
    off("another format");
    assert!(klipolo.got().is_empty());
    // Turned off while the request waited its turn (503), or as the session opened.
    let stopping = program(|got| match got.method().as_str() {
        "initialize" | "notifications/initialized" => like_klipolo(got),
        _ => json_http(
            503,
            &json!({"jsonrpc": "2.0", "id": got.json()["id"], "error": {"code": -32000, "message": "Stopped."}}),
        ),
    });
    file_for(dir.path(), stopping.port, TOKEN, pid);
    off("stopped");
    let stopped = program(|_| http(503, &[], ""));
    file_for(dir.path(), stopped.port, TOKEN, pid);
    off("stopped before the session opened");
}

#[test]
fn writes_nothing_but_json_lines() {
    let k = Klipolo::start();
    let (bridge, out) = bridge(k.dir());
    let mut input = Vec::new();
    for line in [
        "".to_owned(),
        "   ".to_owned(),
        "not json".to_owned(),
        "[1, 2]".to_owned(),
        hello(1, "Agent"),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 9}})
            .to_string(),
        json!({"jsonrpc": "2.0", "id": 7, "result": {}}).to_string(),
        request(2, "ping", json!({})),
        call(3, "getSummary", json!({})),
        json!({"jsonrpc": "2.0", "id": 4}).to_string(),
        request(5, "tools/list", json!({})),
    ] {
        input.extend_from_slice(line.as_bytes());
        input.push(b'\n');
    }
    // Bytes that aren't UTF-8, a line longer than any request Klipolo takes, then a last line with no newline.
    input.extend_from_slice(b"{\"id\": \xff}\r\n");
    input.extend(std::iter::repeat_n(b'x', LINE_LIMIT + 10));
    input.push(b'\n');
    input.extend_from_slice(request(6, "ping", json!({})).as_bytes());
    bridge.serve(Cursor::new(input));

    let text = out.text();
    assert!(text.ends_with('\n'));
    assert!(!text.contains(TOKEN));
    let lines = out.lines();
    for line in &lines {
        assert_eq!(line["jsonrpc"], "2.0", "{line}");
        assert!(
            line.get("result").is_some() != line.get("error").is_some(),
            "{line}"
        );
    }
    let unaddressed: Vec<Value> = (lines.iter())
        .filter(|line| line["id"].is_null())
        .map(|line| line["error"].clone())
        .collect();
    assert_eq!(
        unaddressed,
        [
            json!({"code": -32700, "message": NOT_JSON}),
            json!({"code": -32600, "message": ONE_MESSAGE}),
            json!({"code": -32700, "message": NOT_JSON}),
            json!({"code": -32600, "message": TOO_LARGE}),
        ]
    );
    assert_eq!(lines.len(), unaddressed.len() + 6, "{lines:?}");
    assert_eq!(out.reply(1)["result"], initialize_result("2025-06-18"));
    assert_eq!(out.reply(2)["result"], json!({}));
    assert_eq!(out.reply(3)["result"]["isError"], false);
    assert_eq!(
        out.reply(4)["error"],
        json!({"code": -32600, "message": NO_METHOD})
    );
    assert_eq!(out.reply(5)["result"], listed("tools/list").unwrap());
    assert_eq!(out.reply(6)["result"], json!({}));
}

/// 401: the file is read once more. The run it names now answers; the same file, or a second refusal, is off, and the
/// token goes to each port once.
#[test]
fn a_refused_token_rereads_the_file_once_then_is_off() {
    let pid = std::process::id();
    let dir = tempfile::tempdir().unwrap();
    let (bridge, out) = bridge(dir.path());
    // Hello while there is no file yet, so no session opens before each call.
    bridge.line(&hello(1, "Agent"));
    bridge.wait();
    let refusing = program(|_| http(401, &[("Connection", "close")], ""));
    file_for(dir.path(), refusing.port, TOKEN, pid);
    bridge.line(&call(2, "getSummary", json!({})));
    bridge.wait();
    assert_eq!(out.refused(2), OFF);
    assert_eq!(refusing.methods(), ["initialize"]);

    // Klipolo started again between the read and the request: the file read again names the run that answers.
    let k = Klipolo::start();
    let restarted = connection::read(k.dir()).unwrap();
    let to = dir.path().to_owned();
    let moved = program(move |_| {
        connection::write(&to, &restarted).unwrap();
        http(401, &[], "")
    });
    file_for(dir.path(), moved.port, TOKEN, pid);
    bridge.line(&call(3, "getSummary", json!({})));
    bridge.wait();
    let reply = out.reply(3);
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert_eq!(k.connected(), ["Agent"]);
    assert_eq!(moved.methods().len(), 1);

    // Refused again by the run read the second time: off, no third read.
    let again = program(|_| http(401, &[], ""));
    let to = dir.path().to_owned();
    let second = ConnectionFile {
        port: again.port,
        token: TOKEN.replace('0', "f"),
        pid,
        version: VERSION,
    };
    let first = program(move |_| {
        connection::write(&to, &second).unwrap();
        http(401, &[], "")
    });
    file_for(dir.path(), first.port, TOKEN, pid);
    bridge.line(&call(4, "getSummary", json!({})));
    bridge.wait();
    assert_eq!(out.refused(4), OFF);
    assert_eq!((first.got().len(), again.got().len()), (1, 1));
}

/// 429 on a new session: the call hears how long to wait, and no session is asked for until `Retry-After` passes.
#[test]
fn a_paced_session_waits_out_retry_after() {
    let opened = AtomicUsize::new(0);
    let klipolo = program(move |got| match got.method().as_str() {
        "initialize" if opened.fetch_add(1, Ordering::SeqCst) == 0 => {
            let paced = json!({"jsonrpc": "2.0", "id": got.json()["id"], "error": {"code": -32000, "message": "Wait."}});
            http(
                429,
                &[("Retry-After", "1"), ("Content-Type", "application/json")],
                &paced.to_string(),
            )
        }
        _ => like_klipolo(got),
    });
    let dir = tempfile::tempdir().unwrap();
    file_for(dir.path(), klipolo.port, TOKEN, std::process::id());
    let (bridge, out) = bridge(dir.path());
    bridge.line(&call(1, "getSummary", json!({})));
    bridge.wait();
    let one_second = sessions_paced(Duration::from_secs(1));
    assert_eq!(out.refused(1), one_second);
    bridge.line(&call(2, "getSummary", json!({})));
    bridge.wait();
    assert_eq!(out.refused(2), one_second);
    assert_eq!(
        klipolo.methods(),
        ["initialize"],
        "no new session asked for"
    );
    std::thread::sleep(Duration::from_millis(1100));
    bridge.line(&call(3, "getSummary", json!({})));
    bridge.wait();
    from_klipolo(&out, 3);
    assert_eq!(
        klipolo.methods(),
        [
            "initialize",
            "initialize",
            "notifications/initialized",
            "tools/call"
        ]
    );
}

/// Only a whole reply to the request goes to the client: one too large, cut short, not JSON, for another request, or
/// a redirect (never followed) isn't passed on. Klipolo's own errors are, addressed to the request.
#[test]
fn passes_on_only_a_whole_reply_to_the_request() {
    let elsewhere = program(like_klipolo);
    let away = format!("http://127.0.0.1:{}/mcp", elsewhere.port);
    let klipolo = program(move |got| {
        let msg = got.json();
        let id = msg["id"].clone();
        let tool = msg["params"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let large = |n: u64| {
            let filler = "x".repeat(usize::try_from(n).unwrap());
            json!({"jsonrpc": "2.0", "id": id, "result": tool_result(&filler, false)}).to_string()
        };
        let typed = [("Content-Type", "application/json")];
        match tool.as_str() {
            "large" => http(200, &typed, &large(REPLY_LIMIT)),
            "unsized" => {
                let mut answer = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n".to_vec();
                answer.extend_from_slice(large(REPLY_LIMIT).as_bytes());
                answer
            }
            "nearly" => http(200, &typed, &large(REPLY_LIMIT - 1024)),
            "cut" => {
                let mut answer = http(200, &[], &"{".repeat(100));
                answer.truncate(answer.len() - 50);
                answer
            }
            "text" => http(200, &[], "Not JSON."),
            "another" => json_http(200, &json!({"jsonrpc": "2.0", "id": 99, "result": {}})),
            "redirect" => http(307, &[("Location", away.as_str())], ""),
            "toolarge" => json_http(
                413,
                &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "Too large."}}),
            ),
            _ => like_klipolo(got),
        }
    });
    let dir = tempfile::tempdir().unwrap();
    file_for(dir.path(), klipolo.port, TOKEN, std::process::id());
    let (bridge, out) = bridge(dir.path());
    let tools = [
        "large", "unsized", "nearly", "cut", "text", "another", "redirect", "toolarge",
    ];
    for (n, tool) in tools.iter().enumerate() {
        bridge.line(&call(n as i64, tool, json!({})));
    }
    bridge.wait();
    assert_eq!(out.refused(0), REPLY_TOO_LARGE);
    assert_eq!(out.refused(1), REPLY_TOO_LARGE);
    let nearly = out.reply(2);
    assert_eq!(nearly["result"]["isError"], false);
    // Each went to Klipolo and may have been carried out: the app reads the project before it tries again.
    for id in 3..=6 {
        assert_eq!(out.refused(id), MAY_BE_MADE, "{}", tools[id as usize]);
    }
    assert!(elsewhere.got().is_empty(), "the redirect isn't followed");
    assert_eq!(
        out.reply(7),
        json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32600, "message": "Too large."}})
    );
}

/// A request that went to Klipolo and got no answer (the connection closed, or the time ran out) may still have been
/// carried out: the app hears so, and to read the project before trying again, never a plain "try again" that could
/// add the same words twice. One that never reached Klipolo's handling (its session opening went unanswered) says it
/// stopped.
#[test]
fn a_request_sent_without_an_answer_may_already_be_made() {
    let klipolo = program(|got| match got.method().as_str() {
        "tools/call" => Vec::new(),
        _ => like_klipolo(got),
    });
    let dir = tempfile::tempdir().unwrap();
    file_for(dir.path(), klipolo.port, TOKEN, std::process::id());
    let (bridge, out) = bridge(dir.path());
    let words = json!({"text": "Day 1", "preset": "title", "atSec": null, "durationSec": null});
    bridge.line(&call(1, "addText", words));
    bridge.wait();
    assert_eq!(out.refused(1), MAY_BE_MADE);
    assert!(MAY_BE_MADE.contains("getContext"), "names the read");
    assert_eq!(
        klipolo.methods(),
        ["initialize", "notifications/initialized", "tools/call"],
        "sent once, never again"
    );

    let mute = program(|_| Vec::new());
    let dir = tempfile::tempdir().unwrap();
    file_for(dir.path(), mute.port, TOKEN, std::process::id());
    let (bridge, out) = self::bridge(dir.path());
    bridge.line(&call(2, "getSummary", json!({})));
    bridge.wait();
    assert_eq!(out.refused(2), UNANSWERED);
    assert_eq!(mute.methods(), ["initialize"]);
}

/// Each request on a thread of its own, at most `IN_FLIGHT` at once: one more hears so at once, the rest go on.
#[test]
fn at_most_eight_requests_at_once() {
    assert_eq!(IN_FLIGHT, 8);
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = Arc::clone(&gate);
    let klipolo = program(move |got| {
        if got.method() == "tools/call" {
            let (open, opened) = &*held;
            let mut open = open.lock().unwrap();
            while !*open {
                open = opened.wait(open).unwrap();
            }
        }
        like_klipolo(got)
    });
    let dir = tempfile::tempdir().unwrap();
    file_for(dir.path(), klipolo.port, TOKEN, std::process::id());
    let (bridge, out) = bridge(dir.path());
    bridge.line(&hello(100, "Agent"));
    bridge.wait();
    for id in 0..IN_FLIGHT as i64 {
        bridge.line(&call(id, "getSummary", json!({})));
    }
    bridge.line(&call(8, "getSummary", json!({})));
    // Local answers go on meanwhile.
    bridge.line(&request(9, "ping", json!({})));
    assert_eq!(out.refused(8), BUSY);
    assert_eq!(out.reply(9)["result"], json!({}));
    assert_eq!(out.lines().len(), 3);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    bridge.wait();
    for id in 0..IN_FLIGHT as i64 {
        from_klipolo(&out, id);
    }
    // A slot is free again.
    bridge.line(&call(10, "getSummary", json!({})));
    bridge.wait();
    from_klipolo(&out, 10);
}
