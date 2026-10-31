//! `klipolo-app --mcp`: MCP over stdio, for apps that start a command and talk to it through its stdin and stdout.
//! Each line in is one JSON-RPC message; each answer is one line out, and nothing else is
//! written to stdout.
//!
//! What doesn't depend on the open project the bridge answers by itself: `initialize`, `ping`, the tools, the
//! resources and their template (`rpc::listed`). So an app started before Klipolo, or while other apps' access is off,
//! still has every tool, and a call made then hears how to turn access on (`words::OFF`). Notifications go nowhere:
//! Klipolo acts on none, and no call waits, so there is nothing to cancel.
//!
//! Every other request goes to Klipolo's server, at the port and with the token in `<support>/mcp.json`, read again for
//! each one: Klipolo restarts, and the person turns access off and on, each time with a new port and token. A file
//! whose process is gone counts as off, so a file a crash left behind never sends the token to whatever took its port.
//! The bridge opens a session of its own under the name the app gave in its `initialize`, as soon as it can (so
//! Klipolo shows the app as connected), and again whenever Klipolo no longer knows it (404). A refused token (401)
//! reads the file once more, then counts as off; a server that stopped (503) is off; a pace on new sessions (429) is
//! waited out before the bridge asks for one again.
//!
//! Each forwarded request runs on a thread of its own, at most `IN_FLIGHT` at once. Requests go only to 127.0.0.1,
//! with no proxy, no redirect, a new connection each time (Klipolo closes idle ones), and never an `Origin`. Only a
//! whole reply to the request, at most `REPLY_LIMIT`, is passed on; a request that got none hears so
//! (`words::UNANSWERED`), never silence. The bridge logs nothing, so the token and the headers show nowhere. When its
//! input ends, it waits for the requests in flight, ends its session and returns.

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{ACCEPT, CONTENT_TYPE, RETRY_AFTER};
use serde_json::{Value, json};

use crate::connection::{self, ConnectionFile};
use crate::rpc::{VERSIONS, initialize_result, listed, tool_result};
use crate::server::{BODY_LIMIT, PATH};
use crate::words;

/// Requests forwarded at once; one more hears `words::BUSY`.
pub const IN_FLIGHT: usize = 8;
/// How long reaching Klipolo may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long one request to Klipolo may take, its reply included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest reply passed on (Klipolo's largest, the context, is about 50 KB).
pub const REPLY_LIMIT: u64 = 4 << 20;
/// The longest line read; a longer one is refused unread (Klipolo takes no larger request).
pub const LINE_LIMIT: usize = BODY_LIMIT;
/// The longest pace on new sessions waited out, whatever `Retry-After` says (Klipolo's is a minute at most).
const LONGEST_PACE: Duration = Duration::from_secs(60);
const SESSION_ID: &str = "mcp-session-id";
const PROTOCOL_VERSION: &str = "mcp-protocol-version";
/// The id of the bridge's own `initialize`.
const HELLO_ID: &str = "klipolo-bridge";

/// `klipolo-app --mcp`: the bridge over this process's stdin and stdout, until the app closes stdin. 1 when it can't
/// start: neither `KLIPOLO_SUPPORT_DIR` nor `HOME` names Klipolo's support folder.
pub fn main() -> i32 {
    let Some(dir) = connection::support_dir() else {
        eprintln!("Klipolo's MCP bridge can't find Klipolo's support folder: set HOME.");
        return 1;
    };
    let Some(bridge) = Bridge::new(dir, Box::new(io::stdout())) else {
        eprintln!("Klipolo's MCP bridge couldn't start.");
        return 1;
    };
    bridge.serve(io::stdin().lock());
    0
}

/// The bridge between one app's stdio and Klipolo's server.
pub struct Bridge {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    out: Mutex<Box<dyn Write + Send>>,
    /// Klipolo's support folder, where `mcp.json` is.
    dir: PathBuf,
    /// The app's `initialize` params, sent each time the bridge opens a session with Klipolo.
    hello: Mutex<Value>,
    /// Held while a session opens, so one opens at a time.
    up: Mutex<Upstream>,
    flight: Mutex<Flight>,
    /// Told each time work in flight ends.
    landed: Condvar,
}

/// The bridge's session with Klipolo.
#[derive(Default)]
struct Upstream {
    link: Option<Link>,
    /// Klipolo asked for no new session before this (429).
    paced_until: Option<Instant>,
}

/// A session with one run of Klipolo.
#[derive(Clone, PartialEq)]
struct Link {
    port: u16,
    token: String,
    session: String,
    /// The protocol version Klipolo agreed to.
    version: String,
}

impl Link {
    /// Whether it is a session with the run `file` names.
    fn of(&self, file: &ConnectionFile) -> bool {
        self.port == file.port && self.token == file.token
    }
}

/// Work on threads of the bridge's own: forwarded requests, and all of it (sessions opening too).
#[derive(Default)]
struct Flight {
    requests: usize,
    all: usize,
}

/// Why a request has no reply from Klipolo.
enum Failed {
    /// Klipolo isn't listening, or turned other apps' access off.
    Off,
    /// The token was refused (401).
    Refused,
    /// It went to Klipolo, and no whole reply to it came back.
    Unanswered,
    /// The reply was larger than `REPLY_LIMIT`.
    TooLarge,
    /// New sessions are paced, for this long still.
    Paced(Duration),
}

impl Failed {
    /// What the app hears.
    fn said(&self) -> String {
        match self {
            Failed::Off | Failed::Refused => words::OFF.to_owned(),
            Failed::Unanswered => words::UNANSWERED.to_owned(),
            Failed::TooLarge => words::REPLY_TOO_LARGE.to_owned(),
            Failed::Paced(wait) => words::sessions_paced(*wait),
        }
    }
}

/// What came of one HTTP request.
enum Sent {
    /// Nothing listens on the port.
    NotThere,
    /// It went, and nothing came back.
    Lost,
    Answered {
        status: StatusCode,
        session: Option<String>,
        retry_after: Option<Duration>,
        body: Result<Value, Failed>,
    },
}

impl Bridge {
    /// A bridge to the Klipolo whose support folder is `dir`, answering on `out`. None when its HTTP client can't be
    /// made.
    pub fn new(dir: PathBuf, out: Box<dyn Write + Send>) -> Option<Bridge> {
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        let latest = VERSIONS[VERSIONS.len() - 1];
        Some(Bridge {
            inner: Arc::new(Inner {
                http,
                out: Mutex::new(out),
                dir,
                hello: Mutex::new(json!({"protocolVersion": latest, "capabilities": {}})),
                up: Mutex::default(),
                flight: Mutex::default(),
                landed: Condvar::new(),
            }),
        })
    }

    /// Every line of `input` until it ends (a line longer than `LINE_LIMIT`, or not UTF-8, is refused), then
    /// `finish`.
    pub fn serve(&self, mut input: impl BufRead) {
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = (&mut input)
                .take(LINE_LIMIT as u64 + 1)
                .read_until(b'\n', &mut line);
            if !matches!(read, Ok(1..)) {
                break;
            }
            if line.len() > LINE_LIMIT && line.last() != Some(&b'\n') {
                let rest = input.skip_until(b'\n');
                self.inner
                    .say(&error(&Value::Null, -32600, words::TOO_LARGE));
                if rest.is_err() {
                    break;
                }
                continue;
            }
            match std::str::from_utf8(&line) {
                Ok(text) => self.line(text),
                Err(_) => self
                    .inner
                    .say(&error(&Value::Null, -32700, words::NOT_JSON)),
            }
        }
        self.finish();
    }

    /// One line from the app: one JSON-RPC message, answered here or forwarded on a thread of its own.
    pub fn line(&self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return self
                .inner
                .say(&error(&Value::Null, -32700, words::NOT_JSON));
        };
        let Value::Object(obj) = &msg else {
            return self
                .inner
                .say(&error(&Value::Null, -32600, words::ONE_MESSAGE));
        };
        // A notification: Klipolo acts on none, and nothing waits for an answer.
        let Some(id) = obj.get("id").cloned() else {
            return;
        };
        let Some(method) = obj.get("method").and_then(Value::as_str).map(str::to_owned) else {
            // A reply to a request (the bridge sends none), or a request without a method, which hears why.
            if !obj.contains_key("result") && !obj.contains_key("error") {
                self.inner.say(&error(&id, -32600, words::NO_METHOD));
            }
            return;
        };
        if method == "initialize" {
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            return self.initialize(&id, params);
        }
        if let Some(result) = listed(&method) {
            return self.inner.say(&ok(&id, result));
        }
        self.forward(id, msg, method);
    }

    /// Waits until nothing the bridge started is in flight: no request, and no session opening.
    pub fn wait(&self) {
        let mut flight = locked(&self.inner.flight);
        while flight.all > 0 {
            flight = (self.inner.landed.wait(flight)).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The app is done: waits for the requests in flight, then ends the bridge's session, so Klipolo stops showing
    /// the app as connected.
    pub fn finish(&self) {
        self.wait();
        self.inner.end();
    }

    /// The app's `initialize`, answered here; the session with Klipolo opens now when it can, under the app's name.
    fn initialize(&self, id: &Value, params: Value) {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        *locked(&self.inner.hello) = params;
        self.inner.say(&ok(id, initialize_result(&asked)));
        self.spawn(false, |inner| {
            if let Some(file) = inner.listening() {
                let _ = inner.link(&file);
            }
        });
    }

    /// Request `msg` to Klipolo, on a thread of its own; past `IN_FLIGHT`, it hears `words::BUSY`.
    fn forward(&self, id: Value, msg: Value, method: String) {
        let busy = refusal(&id, &method, words::BUSY);
        let started = self.spawn(true, move |inner| {
            let reply = match inner.reply(&msg, &id) {
                Ok(reply) => reply,
                Err(failed) => refusal(&id, &method, &failed.said()),
            };
            inner.say(&reply);
        });
        if !started {
            self.inner.say(&busy);
        }
    }

    /// `work` on a thread of its own, counted in flight; a request only while fewer than `IN_FLIGHT` are. False when
    /// it didn't start.
    fn spawn(&self, request: bool, work: impl FnOnce(&Inner) + Send + 'static) -> bool {
        {
            let mut flight = locked(&self.inner.flight);
            if request && flight.requests >= IN_FLIGHT {
                return false;
            }
            flight.all += 1;
            flight.requests += usize::from(request);
        }
        let landing = Landing {
            inner: Arc::clone(&self.inner),
            request,
        };
        let started = std::thread::Builder::new()
            .name("klipolo-mcp-bridge".into())
            .spawn(move || work(&landing.inner));
        started.is_ok()
    }
}

/// Counts work as landed when its thread ends, however it ends (or when the thread never started).
struct Landing {
    inner: Arc<Inner>,
    request: bool,
}

impl Drop for Landing {
    fn drop(&mut self) {
        let mut flight = locked(&self.inner.flight);
        flight.all -= 1;
        flight.requests -= usize::from(self.request);
        self.inner.landed.notify_all();
    }
}

impl Inner {
    /// One line to the app. One the app no longer reads goes nowhere: the bridge ends when its input does.
    fn say(&self, reply: &Value) {
        let mut out = locked(&self.out);
        let _ = writeln!(out, "{reply}").and_then(|()| out.flush());
    }

    /// Klipolo's connection file while Klipolo listens: one in Klipolo's format, with a token as Klipolo makes them,
    /// whose process is alive (another user's too).
    fn listening(&self) -> Option<ConnectionFile> {
        connection::read(&self.dir)
            .filter(|f| is_token(&f.token) && klipolo_platform::process::is_alive(f.pid))
    }

    /// Klipolo's reply to request `msg`, whose id is `id`. A session Klipolo forgot opens again once; a refused token
    /// reads the file once more.
    fn reply(&self, msg: &Value, id: &Value) -> Result<Value, Failed> {
        // The run whose token was refused, which the file may name again.
        let mut refused: Option<(u16, String)> = None;
        let mut reopened = false;
        loop {
            let file = self.listening().ok_or(Failed::Off)?;
            if refused
                .as_ref()
                .is_some_and(|(port, token)| *port == file.port && *token == file.token)
            {
                return Err(Failed::Off);
            }
            let link = match self.link(&file) {
                Ok(link) => link,
                Err(Failed::Refused) if refused.is_none() => {
                    refused = Some((file.port, file.token));
                    continue;
                }
                Err(failed) => return Err(failed),
            };
            let (status, body) = match self.post(link.port, &link.token, Some(&link), msg) {
                Sent::NotThere => {
                    self.forget(&link);
                    return Err(Failed::Off);
                }
                Sent::Lost => return Err(Failed::Unanswered),
                Sent::Answered { status, body, .. } => (status, body),
            };
            match status {
                StatusCode::OK => return body.and_then(|body| answering(body, id)),
                StatusCode::UNAUTHORIZED if refused.is_none() => {
                    self.forget(&link);
                    refused = Some((link.port, link.token));
                }
                StatusCode::UNAUTHORIZED | StatusCode::SERVICE_UNAVAILABLE => {
                    self.forget(&link);
                    return Err(Failed::Off);
                }
                StatusCode::NOT_FOUND if !reopened => {
                    self.forget(&link);
                    reopened = true;
                }
                StatusCode::NOT_FOUND => return Err(Failed::Unanswered),
                // Klipolo's own refusal of the request (too large, too slow, failed), addressed to it.
                _ => {
                    let said = body.ok().and_then(|body| body.get("error").cloned());
                    return match said {
                        Some(said @ Value::Object(_)) => {
                            Ok(json!({"jsonrpc": "2.0", "id": id, "error": said}))
                        }
                        _ => Err(Failed::Unanswered),
                    };
                }
            }
        }
    }

    /// The session with the run `file` names: the one the bridge holds, else a new one (`initialize`, then
    /// `notifications/initialized`) under the app's name, unless Klipolo paces new sessions.
    fn link(&self, file: &ConnectionFile) -> Result<Link, Failed> {
        let mut up = locked(&self.up);
        if let Some(link) = up.link.as_ref().filter(|link| link.of(file)) {
            return Ok(link.clone());
        }
        if let Some(until) = up.paced_until {
            let left = until.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                return Err(Failed::Paced(left));
            }
            up.paced_until = None;
        }
        up.link = None;
        let params = locked(&self.hello).clone();
        let hello =
            json!({"jsonrpc": "2.0", "id": HELLO_ID, "method": "initialize", "params": params});
        let (status, session, retry_after, body) =
            match self.post(file.port, &file.token, None, &hello) {
                Sent::NotThere => return Err(Failed::Off),
                Sent::Lost => return Err(Failed::Unanswered),
                Sent::Answered {
                    status,
                    session,
                    retry_after,
                    body,
                } => (status, session, retry_after, body),
            };
        match status {
            StatusCode::OK => {
                let version = (body.ok())
                    .and_then(|body| answering(body, &json!(HELLO_ID)).ok())
                    .and_then(|body| body.pointer("/result/protocolVersion").cloned())
                    .and_then(|version| version.as_str().map(str::to_owned));
                let (Some(session), Some(version)) = (session, version) else {
                    return Err(Failed::Unanswered);
                };
                let link = Link {
                    port: file.port,
                    token: file.token.clone(),
                    session,
                    version,
                };
                // What comes back doesn't matter: Klipolo takes it with nothing to say.
                let ready = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
                self.post(link.port, &link.token, Some(&link), &ready);
                up.link = Some(link.clone());
                Ok(link)
            }
            StatusCode::UNAUTHORIZED => Err(Failed::Refused),
            StatusCode::SERVICE_UNAVAILABLE => Err(Failed::Off),
            StatusCode::TOO_MANY_REQUESTS => {
                let wait = retry_after
                    .unwrap_or(Duration::from_secs(1))
                    .clamp(Duration::from_secs(1), LONGEST_PACE);
                up.paced_until = Some(Instant::now() + wait);
                Err(Failed::Paced(wait))
            }
            _ => Err(Failed::Unanswered),
        }
    }

    /// Drops `link` once Klipolo no longer takes it, unless another request opened a new session meanwhile.
    fn forget(&self, link: &Link) {
        let mut up = locked(&self.up);
        if up.link.as_ref() == Some(link) {
            up.link = None;
        }
    }

    /// Ends the bridge's session, while the file still names its run.
    fn end(&self) {
        let Some(link) = locked(&self.up).link.take() else {
            return;
        };
        if self.listening().is_some_and(|file| link.of(&file)) {
            let ending = self.http.delete(url(link.port));
            let _ = in_session(ending, &link.token, Some(&link)).send();
        }
    }

    /// POSTs `msg` to Klipolo on `port` with `token`, in `link`'s session when given (none for `initialize`).
    fn post(&self, port: u16, token: &str, link: Option<&Link>, msg: &Value) -> Sent {
        let request = (self.http.post(url(port)))
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(msg.to_string());
        let response = match in_session(request, token, link).send() {
            Ok(response) => response,
            Err(e) if e.is_connect() => return Sent::NotThere,
            Err(_) => return Sent::Lost,
        };
        let headers = response.headers();
        let session = (headers.get(SESSION_ID))
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let retry_after = (headers.get(RETRY_AFTER))
            .and_then(|v| v.to_str().ok())
            .and_then(|secs| secs.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        Sent::Answered {
            status: response.status(),
            session,
            retry_after,
            body: whole(response),
        }
    }
}

/// `request` with the token, and with `link`'s session and agreed version when given.
fn in_session(request: RequestBuilder, token: &str, link: Option<&Link>) -> RequestBuilder {
    let request = request.bearer_auth(token);
    match link {
        Some(link) => request
            .header(SESSION_ID, &link.session)
            .header(PROTOCOL_VERSION, &link.version),
        None => request,
    }
}

fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{PATH}")
}

/// A reply's body as JSON, at most `REPLY_LIMIT` of it.
fn whole(response: Response) -> Result<Value, Failed> {
    if response.content_length().is_some_and(|n| n > REPLY_LIMIT) {
        return Err(Failed::TooLarge);
    }
    let mut bytes = Vec::new();
    (response.take(REPLY_LIMIT + 1))
        .read_to_end(&mut bytes)
        .map_err(|_| Failed::Unanswered)?;
    if bytes.len() as u64 > REPLY_LIMIT {
        return Err(Failed::TooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| Failed::Unanswered)
}

/// `reply` when it answers request `id` (a result or an error for it), else none came.
fn answering(reply: Value, id: &Value) -> Result<Value, Failed> {
    let answers = reply.get("id") == Some(id)
        && (reply.get("result").is_some() || reply.get("error").is_some());
    if answers {
        Ok(reply)
    } else {
        Err(Failed::Unanswered)
    }
}

/// A token as Klipolo makes them: 64 lowercase hex digits.
fn is_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// What a request that won't reach Klipolo hears: a tool call, a result an agent reads (`isError`); any other request,
/// an error.
fn refusal(id: &Value, method: &str, said: &str) -> Value {
    if method == "tools/call" {
        ok(id, tool_result(said, true))
    } else {
        error(id, -32000, said)
    }
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
