//! Streamable HTTP on 127.0.0.1: POST `/mcp` with one JSON-RPC message, answered with JSON (no stream: no call waits
//! for the person). A bearer token per run; sessions by `Mcp-Session-Id`.
//!
//! Another program on the Mac can't use Klipolo or wear it down, though it can keep other apps waiting (by holding
//! every connection slot):
//! - It listens on 127.0.0.1 only, on a free port.
//! - Each request is checked from its head, before a byte of its body is read: any `Origin` (a browser page), or a
//!   `Host` other than `127.0.0.1:{port}` or `localhost:{port}` (DNS rebinding), is 403; no token or a wrong one
//!   (`Authorization: Bearer`, compared in constant time) is 401. Both close the connection.
//! - At most `CONNECTIONS` connections are served at once: a slot is taken before a connection is accepted, so the
//!   rest wait unread in the system's queue and hold none of Klipolo's file descriptors.
//! - A head must arrive within `HEAD_TIMEOUT`, which also closes a connection idle that long; a body, once its token
//!   is checked, within `BODY_TIMEOUT` and `BODY_LIMIT`.
//! - Every message is handled on a blocking thread, one at a time for all apps, so the runtime keeps answering.
//!   Once stopped, nothing more is handled: a request still arriving is 503.
//!
//! Then: bad JSON is -32700 (400); `initialize` opens a session (its id in `Mcp-Session-Id`), and every other message
//! needs one (400 without, 404 for one Klipolo doesn't know, so the client opens a new one); too many sessions opened
//! lately is 429 with `Retry-After`. After `initialize`, an `MCP-Protocol-Version` Klipolo doesn't speak is 400, and a
//! request without one is taken at the version agreed. DELETE `/mcp` ends a session (204); GET is 405 (Klipolo sends
//! nothing unasked); any other path is 404.

use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    ALLOW, AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap, HeaderValue,
    ORIGIN, RETRY_AFTER,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::runtime::Handle;
use tokio::sync::{Mutex, Semaphore, watch};

use crate::rpc::{Handled, Mcp, VERSIONS};
use crate::words;

/// Connections served at once; past it, a new one waits in the system's queue until one closes.
pub const CONNECTIONS: usize = 16;
/// How long a request's head may take to arrive, and how long a connection may sit idle between requests.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a request's body may take to arrive, once its token is checked.
pub const BODY_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest body read.
pub const BODY_LIMIT: usize = 256 << 10;
/// How often sessions unused for `session::IDLE` are looked for.
pub const EXPIRE_EVERY: Duration = Duration::from_secs(60);
/// How long a connection may finish the request it is on once the server stops.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// The one path served.
pub const PATH: &str = "/mcp";
const SESSION_ID: &str = "mcp-session-id";
const PROTOCOL_VERSION: &str = "mcp-protocol-version";

type Answer = Response<Full<Bytes>>;

/// The server while it runs. Dropping it stops it, as `stop` does.
pub struct Running {
    pub port: u16,
    stop: watch::Sender<bool>,
    mcp: Arc<Mcp>,
}

impl Running {
    /// Stops: the accept loop ends and the port closes, each connection finishes the request it is on (for at most
    /// `STOP_GRACE`) and closes, and every session ends. The paces and the app's waiting proposal stay with the `Mcp`.
    pub fn stop(self) {
        drop(self);
    }

    /// The apps connected now, each once, sorted.
    pub fn connected(&self) -> Vec<String> {
        self.mcp.connected()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        self.mcp.end_all();
    }
}

/// What every connection shares.
struct Shared {
    mcp: Arc<Mcp>,
    token: String,
    port: u16,
    /// Held while a message is handled: one at a time, for all apps.
    turn: Arc<Mutex<()>>,
    /// True once the server stops.
    stopping: watch::Receiver<bool>,
}

impl Shared {
    fn stopped(&self) -> bool {
        *self.stopping.borrow()
    }
}

/// Listens on a free port of 127.0.0.1 now, so a failure is said at once, and serves on `rt` until stopped.
pub fn serve(rt: &Handle, mcp: Arc<Mcp>, token: String) -> io::Result<Running> {
    let listener = std::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let listener = {
        let _in = rt.enter();
        tokio::net::TcpListener::from_std(listener)?
    };
    let (stop, stopping) = watch::channel(false);
    let shared = Arc::new(Shared {
        mcp: Arc::clone(&mcp),
        token,
        port,
        turn: Arc::new(Mutex::new(())),
        stopping: stopping.clone(),
    });
    rt.spawn(accept(listener, Arc::clone(&shared), stopping.clone()));
    rt.spawn(expire(shared, stopping));
    tracing::info!(port, "the MCP server is listening");
    Ok(Running { port, stop, mcp })
}

/// Serves connections until stopped, at most `CONNECTIONS` at once: a slot is taken before a connection is accepted.
/// Each connection is HTTP/1 with a timer, so a head slower than `HEAD_TIMEOUT`, or that long idle, closes it.
async fn accept(
    listener: tokio::net::TcpListener,
    s: Arc<Shared>,
    mut stopping: watch::Receiver<bool>,
) {
    let slots = Arc::new(Semaphore::new(CONNECTIONS));
    loop {
        let slot = tokio::select! {
            slot = Arc::clone(&slots).acquire_owned() => slot,
            _ = stopping.wait_for(|stop| *stop) => return,
        };
        let Ok(slot) = slot else { return };
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stopping.wait_for(|stop| *stop) => return,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            // Out of descriptors, or a connection reset before it was taken: the slot goes back, and a moment later
            // the next one is tried (never a busy loop).
            Err(error) => {
                tracing::debug!(%error, "an MCP connection couldn't be accepted");
                drop(slot);
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let s = Arc::clone(&s);
        let mut stopped = stopping.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let service = service_fn(move |req| {
                let s = Arc::clone(&s);
                async move { Ok::<_, Infallible>(answer(&s, req).await) }
            });
            let connection = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEAD_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service);
            tokio::pin!(connection);
            let stopping = tokio::select! {
                _ = connection.as_mut() => false,
                _ = stopped.wait_for(|stop| *stop) => true,
            };
            if stopping {
                connection.as_mut().graceful_shutdown();
                let _ = tokio::time::timeout(STOP_GRACE, connection).await;
            }
        });
    }
}

/// Every `EXPIRE_EVERY` until stopped, the sessions unused for `session::IDLE` go.
async fn expire(s: Arc<Shared>, mut stopping: watch::Receiver<bool>) {
    let first = tokio::time::Instant::now() + EXPIRE_EVERY;
    let mut every = tokio::time::interval_at(first, EXPIRE_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = every.tick() => {}
            _ = stopping.wait_for(|stop| *stop) => return,
        }
        let mcp = Arc::clone(&s.mcp);
        blocking(&s, move || mcp.expire(mcp.now())).await;
    }
}

/// `f` on a blocking thread, in its turn: one at a time for all apps. None when it panicked, or when the server
/// stopped before its turn came: once the person turns it off, nothing more is handled.
async fn blocking<T: Send + 'static>(
    s: &Shared,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let turn = Arc::clone(&s.turn).lock_owned().await;
    if s.stopped() {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        let _turn = turn;
        f()
    })
    .await
    .ok()
}

/// One request, checked from its head first.
async fn answer(s: &Shared, req: Request<Incoming>) -> Answer {
    if foreign(s.port, req.headers()) {
        return closing(StatusCode::FORBIDDEN);
    }
    if !authorized(&s.token, req.headers()) {
        return closing(StatusCode::UNAUTHORIZED);
    }
    if req.uri().path() != PATH {
        return closing(StatusCode::NOT_FOUND);
    }
    match *req.method() {
        Method::POST => post(s, req).await,
        Method::DELETE => delete(s, req.headers()).await,
        _ => {
            let mut answer = closing(StatusCode::METHOD_NOT_ALLOWED);
            (answer.headers_mut()).insert(ALLOW, HeaderValue::from_static("POST, DELETE"));
            answer
        }
    }
}

/// A browser's request (any `Origin`), or one for a host other than ours (DNS rebinding), or with no one `Host`.
fn foreign(port: u16, headers: &HeaderMap) -> bool {
    if headers.contains_key(ORIGIN) {
        return true;
    }
    let mut hosts = headers.get_all(HOST).iter();
    let (Some(host), None) = (hosts.next(), hosts.next()) else {
        return true;
    };
    let Ok(host) = host.to_str() else {
        return true;
    };
    let ours = |name: &str| host.eq_ignore_ascii_case(&format!("{name}:{port}"));
    !(ours("127.0.0.1") || ours("localhost"))
}

/// Whether the request carries this run's token: `Authorization: Bearer <token>`, compared in time that doesn't
/// depend on where it differs.
fn authorized(token: &str, headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some((scheme, given)) = value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer")
        && bool::from(given.trim().as_bytes().ct_eq(token.as_bytes()))
}

/// Whether a request's `MCP-Protocol-Version` is one Klipolo speaks; one without it is taken at the version agreed.
fn speaks(headers: &HeaderMap) -> bool {
    headers
        .get(PROTOCOL_VERSION)
        .is_none_or(|v| v.to_str().is_ok_and(|v| VERSIONS.contains(&v)))
}

/// POST: one message, read within the limits, handled in its turn.
async fn post(s: &Shared, req: Request<Incoming>) -> Answer {
    let session =
        (req.headers().get(SESSION_ID)).map(|v| v.to_str().unwrap_or_default().to_owned());
    let speaks = speaks(req.headers());
    let claimed = (req.headers().get(CONTENT_LENGTH))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if claimed.is_some_and(|n| n > BODY_LIMIT as u64) {
        return refused(StatusCode::PAYLOAD_TOO_LARGE, words::TOO_LARGE);
    }
    let read = Limited::new(req.into_body(), BODY_LIMIT).collect();
    let body = match tokio::time::timeout(BODY_TIMEOUT, read).await {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(e)) if e.downcast_ref::<LengthLimitError>().is_some() => {
            return refused(StatusCode::PAYLOAD_TOO_LARGE, words::TOO_LARGE);
        }
        // The body broke off: the client is gone.
        Ok(Err(_)) => return closing(StatusCode::BAD_REQUEST),
        Err(_) => return refused(StatusCode::REQUEST_TIMEOUT, words::TOO_SLOW),
    };
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return json_answer(
            StatusCode::BAD_REQUEST,
            &rpc_error(&Value::Null, -32700, words::NOT_JSON),
        );
    };
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    // `initialize` agrees on the version in its body; every later request names it, or is taken at it.
    if !speaks && msg.get("method").and_then(Value::as_str) != Some("initialize") {
        return json_answer(
            StatusCode::BAD_REQUEST,
            &rpc_error(&id, -32600, words::VERSION_UNKNOWN),
        );
    }
    let known = session.is_some();
    let mcp = Arc::clone(&s.mcp);
    let Some(handled) = blocking(s, move || mcp.handle(session.as_deref(), msg)).await else {
        return not_handled(s, &id);
    };
    match handled {
        Handled::Reply { body, new_session } => {
            // The server stopped while this opened a session: it ends with the rest.
            if let Some(id) = &new_session
                && s.stopped()
            {
                s.mcp.end(id);
            }
            let mut answer = json_answer(StatusCode::OK, &body);
            if let Some(v) = new_session.and_then(|id| HeaderValue::from_str(&id).ok()) {
                answer.headers_mut().insert(SESSION_ID, v);
            }
            answer
        }
        Handled::Accepted => empty(StatusCode::ACCEPTED),
        Handled::NoSession if known => json_answer(
            StatusCode::NOT_FOUND,
            &rpc_error(&id, -32001, words::NO_SUCH_SESSION),
        ),
        Handled::NoSession => json_answer(
            StatusCode::BAD_REQUEST,
            &rpc_error(&id, -32600, words::SESSION_FIRST),
        ),
        Handled::Paced(wait) => {
            let mut answer = json_answer(
                StatusCode::TOO_MANY_REQUESTS,
                &rpc_error(&id, -32000, &words::sessions_paced(wait)),
            );
            let secs = (wait.as_secs_f64().ceil() as u64).max(1);
            answer
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(secs));
            answer
        }
    }
}

/// DELETE: the client ends its session.
async fn delete(s: &Shared, headers: &HeaderMap) -> Answer {
    if !speaks(headers) {
        return json_answer(
            StatusCode::BAD_REQUEST,
            &rpc_error(&Value::Null, -32600, words::VERSION_UNKNOWN),
        );
    }
    let Some(session) = headers.get(SESSION_ID) else {
        return json_answer(
            StatusCode::BAD_REQUEST,
            &rpc_error(&Value::Null, -32600, words::SESSION_FIRST),
        );
    };
    let session = session.to_str().unwrap_or_default().to_owned();
    let mcp = Arc::clone(&s.mcp);
    match blocking(s, move || mcp.end(&session)).await {
        Some(true) => empty(StatusCode::NO_CONTENT),
        Some(false) => json_answer(
            StatusCode::NOT_FOUND,
            &rpc_error(&Value::Null, -32001, words::NO_SUCH_SESSION),
        ),
        None => not_handled(s, &Value::Null),
    }
}

/// A message that wasn't handled: the server stopped (503), or handling it failed (500).
fn not_handled(s: &Shared, id: &Value) -> Answer {
    if s.stopped() {
        let mut answer = json_answer(
            StatusCode::SERVICE_UNAVAILABLE,
            &rpc_error(id, -32000, words::STOPPED),
        );
        (answer.headers_mut()).insert(CONNECTION, HeaderValue::from_static("close"));
        return answer;
    }
    json_answer(
        StatusCode::INTERNAL_SERVER_ERROR,
        &rpc_error(id, -32603, words::FAILED),
    )
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn json_answer(status: StatusCode, body: &Value) -> Answer {
    let mut answer = Response::new(Full::new(Bytes::from(body.to_string())));
    *answer.status_mut() = status;
    (answer.headers_mut()).insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    answer
}

fn empty(status: StatusCode) -> Answer {
    let mut answer = Response::new(Full::new(Bytes::new()));
    *answer.status_mut() = status;
    answer
}

/// `status` with no body, and the connection closes with it: a refused client keeps no slot.
fn closing(status: StatusCode) -> Answer {
    let mut answer = empty(status);
    (answer.headers_mut()).insert(CONNECTION, HeaderValue::from_static("close"));
    answer
}

/// `status` with `said` as a JSON-RPC error, and the connection closes with it.
fn refused(status: StatusCode, said: &str) -> Answer {
    let mut answer = json_answer(status, &rpc_error(&Value::Null, -32600, said));
    (answer.headers_mut()).insert(CONNECTION, HeaderValue::from_static("close"));
    answer
}
