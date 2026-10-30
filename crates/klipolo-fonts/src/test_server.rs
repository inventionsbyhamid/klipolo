//! A local stand-in for Google's files (the `test-hosts` feature, tests only): this crate's download tests and the
//! app's command tests ask it, never Google. It serves the bundled fonts as Google's files (`/s/<tail>`, tails like
//! `Inter/Inter-Regular.ttf`, so validation runs on real faces). It counts requests and the most it served at once, can
//! drop a connection after n bytes of a file, say another weight in a file's OS/2 table, wait before answering, and
//! answers 404 for anything else.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use crate::google::{GoogleFamily, GoogleFile};

#[derive(Default)]
struct State {
    /// Every path asked for, in order.
    asked: Vec<String>,
    /// A path containing this sends only this many bytes of its body, then closes.
    drop: Vec<(String, usize)>,
    /// A path containing this is sent with this OS/2 weight (`usWeightClass`).
    weight: Vec<(String, u16)>,
    /// How long each answer waits (ms).
    delay_ms: u64,
    /// Answers being sent now, and the most at once.
    in_flight: usize,
    most: usize,
}

/// The server, on 127.0.0.1 at a port of its own, until it is dropped (its thread then serves nothing more).
pub struct Server {
    port: u16,
    state: Arc<Mutex<State>>,
}

impl Server {
    pub fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
        let port = listener.local_addr().expect("its address").port();
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                std::thread::spawn(move || serve(stream, &state));
            }
        });
        Server { port, state }
    }

    /// `http://127.0.0.1:<port>/`: what a family made by `family_on` downloads from.
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// From now on a file whose path contains `name` sends `bytes` of itself, then the connection drops.
    pub fn drop_after_bytes(&self, name: &str, bytes: usize) {
        self.lock().drop.push((name.to_owned(), bytes));
    }

    /// Every file is sent whole again.
    pub fn heal(&self) {
        self.lock().drop.clear();
    }

    /// From now on a file whose path contains `name` says it is `weight` (its OS/2 `usWeightClass`).
    pub fn say_weight(&self, name: &str, weight: u16) {
        self.lock().weight.push((name.to_owned(), weight));
    }

    /// Each answer waits `ms` first.
    pub fn delay(&self, ms: u64) {
        self.lock().delay_ms = ms;
    }

    /// The most answers it was sending at once.
    pub fn most_at_once(&self) -> usize {
        self.lock().most
    }

    /// Requests for paths containing `name` so far.
    pub fn requests(&self, name: &str) -> usize {
        self.lock()
            .asked
            .iter()
            .filter(|p| p.contains(name))
            .count()
    }

    /// Every request so far.
    pub fn total_requests(&self) -> usize {
        self.lock().asked.len()
    }

    /// Every path asked for, in order.
    pub fn paths(&self) -> Vec<String> {
        self.lock().asked.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn bundled() -> PathBuf {
    crate::bundled::bundled_dir().expect("the bundled fonts folder")
}

fn serve(stream: TcpStream, state: &Mutex<State>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    // The headers, up to the blank line.
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(_) => {}
        }
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let lock = || state.lock().unwrap_or_else(PoisonError::into_inner);
    let (drop, weight, delay) = {
        let mut s = lock();
        s.asked.push(path.clone());
        s.in_flight += 1;
        s.most = s.most.max(s.in_flight);
        let found = |list: &[(String, usize)]| {
            list.iter()
                .find(|(name, _)| path.contains(name.as_str()))
                .map(|&(_, n)| n)
        };
        let weight = s
            .weight
            .iter()
            .find(|(name, _)| path.contains(name.as_str()))
            .map(|&(_, w)| w);
        (found(&s.drop), weight, s.delay_ms)
    };
    if delay > 0 {
        std::thread::sleep(std::time::Duration::from_millis(delay));
    }
    let body = path
        .strip_prefix("/s/")
        .filter(|tail| !tail.contains(".."))
        .and_then(|tail| std::fs::read(bundled().join(tail)).ok())
        .map(|mut body| {
            if let Some(w) = weight {
                say_weight(&mut body, w);
            }
            body
        });
    let mut out = stream;
    match body {
        Some(body) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = out.write_all(head.as_bytes());
            let sent = drop.unwrap_or(body.len()).min(body.len());
            let _ = out.write_all(&body[..sent]);
            let _ = out.flush();
            if drop.is_some() {
                let _ = out.shutdown(Shutdown::Both);
            }
        }
        None => {
            let _ = out.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    }
    lock().in_flight -= 1;
}

/// Sets the OS/2 table's `usWeightClass` of the font in `body` (its table directory read as sfnt lays it out).
fn say_weight(body: &mut [u8], weight: u16) {
    let u32_at =
        |b: &[u8], o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
    let tables = u16::from_be_bytes([body[4], body[5]]) as usize;
    for i in 0..tables {
        let record = 12 + 16 * i;
        if &body[record..record + 4] == b"OS/2" {
            let offset = u32_at(body, record + 8);
            body[offset + 4..offset + 6].copy_from_slice(&weight.to_be_bytes());
        }
    }
}

/// A Google family named `family` at `v1`, licensed OFL, whose files `(weight, italic, bundled tail)` the server
/// serves (each its real size, as the catalogue says).
pub fn family_on(server: &Server, family: &str, files: &[(u16, bool, &str)]) -> GoogleFamily {
    family_on_version(server, family, "v1", files)
}

/// `family_on` at `version`.
pub fn family_on_version(
    server: &Server,
    family: &str,
    version: &str,
    files: &[(u16, bool, &str)],
) -> GoogleFamily {
    let files = files
        .iter()
        .map(|&(weight, italic, tail)| GoogleFile {
            weight,
            italic,
            url_tail: tail.to_owned(),
            bytes: std::fs::metadata(bundled().join(tail)).map_or(0, |m| m.len()),
        })
        .collect();
    GoogleFamily::for_tests(family, version, files, Some(server.base()))
}
