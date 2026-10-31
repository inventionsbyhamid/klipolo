//! A client of Klipolo's MCP server in a test: JSON-RPC over HTTP (reqwest), and raw requests over TCP for what a
//! well-behaved client never sends (a head that claims a body and stops, a foreign `Host`).

#![allow(dead_code)]

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const SESSION_ID: &str = "mcp-session-id";

pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// `initialize`'s params for the app `name`.
pub fn hello(name: &str) -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": name, "version": "1.0"},
    })
}

/// An app connected over HTTP: where the server listens, the token it holds, and its session once open.
pub struct Client {
    http: reqwest::Client,
    pub url: String,
    pub token: String,
    pub session: Option<String>,
}

impl Client {
    /// A new connection for each request, so no idle connection outlives the server's timers.
    pub fn new(port: u16, token: &str) -> Client {
        let http = reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap();
        Client {
            http,
            url: format!("http://127.0.0.1:{port}/mcp"),
            token: token.to_owned(),
            session: None,
        }
    }

    /// POSTs `body` as JSON, with the token, and the session when open.
    pub async fn post_bytes(&self, body: Vec<u8>) -> reqwest::Response {
        let mut req = (self.http.post(&self.url))
            .bearer_auth(&self.token)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body);
        if let Some(session) = &self.session {
            req = req.header(SESSION_ID, session);
        }
        req.send().await.expect("the server answers")
    }

    pub async fn post(&self, msg: &Value) -> reqwest::Response {
        self.post_bytes(msg.to_string().into_bytes()).await
    }

    /// Opens a session as the app `name` and says it is ready: `initialize`'s result.
    pub async fn initialize(&mut self, name: &str) -> Value {
        let answer = self.post(&request(1, "initialize", hello(name))).await;
        assert_eq!(answer.status(), StatusCode::OK);
        let session = (answer.headers().get(SESSION_ID))
            .expect("a session id")
            .to_str()
            .unwrap()
            .to_owned();
        let body: Value = answer.json().await.unwrap();
        self.session = Some(session);
        let ready = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert_eq!(self.post(&ready).await.status(), StatusCode::ACCEPTED);
        body["result"].clone()
    }

    /// `method` with `params`, answered 200: the reply.
    pub async fn ask(&self, method: &str, params: Value) -> Value {
        let answer = self.post(&request(2, method, params)).await;
        assert_eq!(answer.status(), StatusCode::OK, "{method}");
        answer.json().await.unwrap()
    }

    /// Tool `name`'s result.
    pub async fn call(&self, name: &str, arguments: Value) -> Value {
        let reply = (self.ask("tools/call", json!({"name": name, "arguments": arguments}))).await;
        reply["result"].clone()
    }

    /// Resource `uri`'s result.
    pub async fn read(&self, uri: &str) -> Value {
        self.ask("resources/read", json!({"uri": uri})).await["result"].clone()
    }

    /// Ends the session (DELETE): the status.
    pub async fn end(&self) -> StatusCode {
        let mut req = self.http.delete(&self.url).bearer_auth(&self.token);
        if let Some(session) = &self.session {
            req = req.header(SESSION_ID, session);
        }
        req.send().await.expect("the server answers").status()
    }

    /// Whether the server answers a ping in this session within a second.
    pub async fn answers(&self) -> bool {
        let ping = request(3, "ping", json!({}));
        match tokio::time::timeout(Duration::from_secs(1), self.post(&ping)).await {
            Ok(answer) => answer.status() == StatusCode::OK,
            Err(_) => false,
        }
    }
}

/// What came back on a raw connection: the status line's code (0 when nothing came), the head, the body, and
/// whether the server closed the connection.
#[derive(Debug)]
pub struct Raw {
    pub status: u16,
    pub head: String,
    pub body: String,
    pub closed: bool,
}

impl Raw {
    /// Header `name`'s value, in any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        (self.head.lines().skip(1))
            .filter_map(|line| line.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

/// A request's head: `method path`, then each header, then the blank line.
pub fn head(method: &str, path: &str, headers: &[(&str, String)]) -> String {
    let mut head = format!("{method} {path} HTTP/1.1\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head + "\r\n"
}

/// Sends `bytes` on a new connection to `port`, then reads what comes back until the server closes it or `within`
/// passes.
pub async fn raw(port: u16, bytes: &[u8], within: Duration) -> Raw {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(bytes).await.unwrap();
    read_raw(&mut stream, within).await
}

/// Reads from `stream` until the server closes it or `within` passes.
pub async fn read_raw(stream: &mut TcpStream, within: Duration) -> Raw {
    let mut got = Vec::new();
    let closed = tokio::time::timeout(within, async {
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
            }
        }
    })
    .await
    .is_ok();
    let text = String::from_utf8_lossy(&got).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = (head.split(' ').nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    Raw {
        status,
        head: head.to_owned(),
        body: body.to_owned(),
        closed,
    }
}
