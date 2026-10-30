//! Frame server. It accepts one authenticated websocket client on 127.0.0.1, and the newest frame wins.
//! Websockets bypass CORS, so any page or local process could otherwise read the user's frames. That is why:
//! - a 256-bit per-run token is required (it reaches the webview only through Tauri IPC);
//! - the Origin must be on the allowlist;
//! - the Host must be the loopback literal.
//!
//! Never log request URIs: they contain the token.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::serve::ListenerExt;
use bytes::Bytes;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};

/// Largest message (and frame) a client may send; clients have nothing to say, so anything they send is ignored.
const MAX_CLIENT_MESSAGE: usize = 1024;
/// The client generation once the server has stopped: every client closes and none may join.
const STOPPED: u64 = u64::MAX;
/// How long a leaving client gets to take the close frame before its socket is dropped.
const CLOSE_GRACE: Duration = Duration::from_secs(1);

/// Where the engine puts finished frames. Publishing replaces any frame the client hasn't been sent yet.
#[derive(Clone)]
pub struct FrameSink {
    frames: Arc<watch::Sender<Option<Bytes>>>,
}

impl FrameSink {
    pub fn publish(&self, frame: Bytes) {
        self.frames.send_replace(Some(frame));
    }
}

/// The running server. Dropping it stops accepting connections and closes the connected client.
pub struct FrameServer {
    pub port: u16,
    pub token: String,
    generation: Arc<watch::Sender<u64>>,
    stop: Option<oneshot::Sender<()>>,
}

#[derive(Clone)]
struct Shared {
    token: Arc<str>,
    origins: Arc<[String]>,
    host: Arc<str>,
    frames: watch::Receiver<Option<Bytes>>,
    /// Bumped by each accepted client; older pumps see the change and close.
    generation: Arc<watch::Sender<u64>>,
}

impl FrameServer {
    pub async fn start(allowed_origins: Vec<String>) -> std::io::Result<(FrameServer, FrameSink)> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let port = listener.local_addr()?.port();
        let token = new_token();
        let (tx, rx) = watch::channel(None);
        let generation = Arc::new(watch::channel(0).0);
        let shared = Shared {
            token: token.as_str().into(),
            origins: allowed_origins.into(),
            host: format!("127.0.0.1:{port}").into(),
            frames: rx,
            generation: generation.clone(),
        };
        let app = Router::new()
            .route("/frames", get(upgrade))
            .with_state(shared);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let shutdown = async {
                let _ = stop_rx.await;
            };
            // No Nagle: each frame's last partial segment goes out at once instead of waiting for an ACK.
            let listener = listener.tap_io(|tcp| {
                let _ = tcp.set_nodelay(true);
            });
            if let Err(error) = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await
            {
                tracing::warn!(%error, "frame server stopped");
            }
        });
        let server = FrameServer {
            port,
            token,
            generation,
            stop: Some(stop_tx),
        };
        Ok((
            server,
            FrameSink {
                frames: Arc::new(tx),
            },
        ))
    }
}

impl Drop for FrameServer {
    fn drop(&mut self) {
        // Upgraded sockets outlive axum's graceful shutdown, so close them through the generation.
        self.generation.send_replace(STOPPED);
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("system randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compare without an early exit, so timing doesn't reveal how much of a guess was right.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn header_matches(
    headers: &HeaderMap,
    name: header::HeaderName,
    ok: impl Fn(&str) -> bool,
) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .is_some_and(ok)
}

async fn upgrade(
    State(s): State<Shared>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let authorized = query.get("t").is_some_and(|t| same(t, &s.token))
        && header_matches(&headers, header::ORIGIN, |o| {
            s.origins.iter().any(|a| a == o)
        })
        && header_matches(&headers, header::HOST, |h| h == &*s.host);
    if !authorized {
        // The Origin is the one header worth naming when a webview is refused (never the URI: it holds the token).
        let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        tracing::warn!(?origin, "frame socket refused");
        if cfg!(debug_assertions) {
            eprintln!("klipolo: frame socket refused (origin {origin:?})");
        }
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut mine = None;
    s.generation.send_if_modified(|g| {
        if *g == STOPPED {
            return false;
        }
        *g += 1;
        mine = Some(*g);
        true
    });
    let Some(mine) = mine else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.max_message_size(MAX_CLIENT_MESSAGE)
        .max_frame_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| pump(socket, s, mine))
}

async fn pump(mut socket: WebSocket, s: Shared, mine: u64) {
    let mut frames = s.frames.clone();
    let mut generation = s.generation.subscribe();
    // Checked after subscribing, so a newer client (or a stop) that arrived before this task started still ends it.
    if *generation.borrow_and_update() == mine {
        send_newest(&mut socket, &mut frames, &mut generation, mine).await;
    }
    // A client that stopped reading must not hold this task (or the server's shutdown) forever.
    let _ = tokio::time::timeout(CLOSE_GRACE, socket.send(Message::Close(None))).await;
}

/// Sends one frame, unless a newer client replaces this one or the server stops first: a client that stopped
/// reading would otherwise block replacement and shutdown. False when the client should be closed.
async fn send_frame(
    socket: &mut WebSocket,
    frame: Bytes,
    generation: &mut watch::Receiver<u64>,
    mine: u64,
) -> bool {
    tokio::select! {
        sent = socket.send(Message::Binary(frame)) => sent.is_ok(),
        () = replaced(generation, mine) => false,
    }
}

/// Resolves once this client is no longer the current one.
async fn replaced(generation: &mut watch::Receiver<u64>, mine: u64) {
    loop {
        if generation.changed().await.is_err() || *generation.borrow_and_update() != mine {
            return;
        }
    }
}

/// Sends the newest frame whenever one is published, until the client leaves, a newer client replaces it, the
/// server stops or the sink is dropped.
async fn send_newest(
    socket: &mut WebSocket,
    frames: &mut watch::Receiver<Option<Bytes>>,
    generation: &mut watch::Receiver<u64>,
    mine: u64,
) {
    let first = frames.borrow_and_update().clone();
    if let Some(frame) = first
        && !send_frame(socket, frame, generation, mine).await
    {
        return;
    }
    loop {
        tokio::select! {
            changed = frames.changed() => {
                if changed.is_err() {
                    break;
                }
                let frame = frames.borrow_and_update().clone();
                if let Some(frame) = frame
                    && !send_frame(socket, frame, generation, mine).await
                {
                    break;
                }
            }
            changed = generation.changed() => {
                if changed.is_err() || *generation.borrow_and_update() != mine {
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {} // clients have nothing to say; their messages are ignored
            },
        }
    }
}
