//! The one way a download reaches the network: Google's static files only, through one shared HTTP
//! client, with every request checked against the cancel flag first.
//!
//! - **One host.** A URL is `https://fonts.gstatic.com/s/` plus a tail from the catalogue, and the join must keep that
//!   base's scheme, host, port and path: a tail with `..`, `//`, a scheme, a query or an escape is refused before any
//!   request. Redirects (at most 3) must stay on the host and port asked. No referrer, no cookies, no credentials, the
//!   user agent "Klipolo", the system's proxy. The `test-hosts` feature (tests only) points the base at a local server
//!   the family names, and never at Google.
//! - **One client**, built once on the first (blocking) thread that needs it and shared after: the system's
//!   certificates are read once, and connections are reused.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use klipolo_platform::durable::{self, Durability};
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::redirect;
use sha2::{Digest, Sha256};

use super::FontError;
use super::check::hex;
use crate::google::GoogleFamily;

/// Where font files come from: Google's static files.
#[cfg_attr(feature = "test-hosts", allow(dead_code))]
const FILES: &str = crate::google::GSTATIC;
/// A preview file is a few KB; nothing bigger is taken.
pub(super) const MAX_SMALL_BYTES: u64 = 1 << 20;
/// Tries after the first on a lost connection, before `Offline`.
pub(super) const RETRIES: u32 = 2;
/// Progress is told at most every this many bytes (and at the start and end).
const PROGRESS_EVERY: u64 = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 3;
const CHUNK: usize = 64 * 1024;

/// The base a family's files come from.
pub(super) struct Hosts {
    pub(super) files: Url,
    https: bool,
}

impl Hosts {
    /// Google's. Under `test-hosts`, the local server the family names (else a closed local port): never Google.
    pub(super) fn of(fam: &GoogleFamily) -> Hosts {
        #[cfg(feature = "test-hosts")]
        {
            let base = fam
                .test_base
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:9/".into());
            Hosts {
                files: Url::parse(&format!("{base}s/")).expect("a test base"),
                https: false,
            }
        }
        #[cfg(not(feature = "test-hosts"))]
        {
            let _ = fam;
            Hosts {
                files: Url::parse(FILES).expect("Google's files"),
                https: true,
            }
        }
    }

    /// `tail` under the files' base.
    pub(super) fn file(&self, tail: &str) -> Result<Url, FontError> {
        url(&self.files, tail)
    }
}

/// `base` joined with `tail`, which must stay under it: the same scheme, host, port and path prefix, with nothing that
/// could climb out of it or name another host (`..`, `//`, a scheme, a query, an escape). Else `BadFile("address")`.
pub(super) fn url(base: &Url, tail: &str) -> Result<Url, FontError> {
    let address = || FontError::BadFile("address".into());
    let plain = !tail.is_empty()
        && !tail.starts_with('/')
        && !tail.contains("..")
        && !tail.contains("//")
        && tail
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'));
    if !plain {
        return Err(address());
    }
    let joined = base.join(tail).map_err(|_| address())?;
    let same = joined.scheme() == base.scheme()
        && joined.host_str() == base.host_str()
        && joined.port_or_known_default() == base.port_or_known_default()
        && joined.path().starts_with(base.path())
        && joined.query().is_none()
        && joined.fragment().is_none()
        && joined.username().is_empty()
        && joined.password().is_none();
    if same { Ok(joined) } else { Err(address()) }
}

fn build(https: bool) -> Result<Client, FontError> {
    let policy = || {
        redirect::Policy::custom(move |attempt| {
            let from = attempt.previous().first().cloned();
            let to = attempt.url();
            let stays = from.as_ref().is_some_and(|from| {
                from.host_str() == to.host_str()
                    && from.port_or_known_default() == to.port_or_known_default()
            }) && (to.scheme() == "https" || !https)
                && to.username().is_empty()
                && to.password().is_none();
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if stays {
                attempt.follow()
            } else {
                attempt.error("a redirect to another host")
            }
        })
    };
    let make = |native_roots: bool| {
        let b = Client::builder()
            .redirect(policy())
            .referer(false)
            .user_agent("Klipolo")
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .https_only(https)
            .tls_built_in_native_certs(native_roots);
        // Tests talk to 127.0.0.1 straight: a developer's or CI's HTTP_PROXY never carries them.
        let b = if https { b } else { b.no_proxy() };
        b.build()
    };
    make(true)
        .or_else(|e| {
            tracing::warn!(error = %e, "fonts: the system's certificates couldn't be read; using the built-in ones");
            make(false)
        })
        .map_err(|_| FontError::Offline)
}

/// The shared client, built on first use (on a blocking thread: it reads the system's certificates). Clones share
/// its connections. Tests' local servers share one plain-HTTP client.
pub(super) fn client(hosts: &Hosts) -> Result<Client, FontError> {
    static GOOGLE: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    static LOCAL: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    let held = if hosts.https { &GOOGLE } else { &LOCAL };
    if let Some(c) = held.get() {
        return Ok(c.clone());
    }
    let made = build(hosts.https)?;
    Ok(held.get_or_init(|| made).clone())
}

/// Bytes done of the total, told at most every `PROGRESS_EVERY`.
pub(super) struct Progress<'a> {
    pub(super) tell: &'a (dyn Fn(u64, u64) + Sync),
    pub(super) done: u64,
    pub(super) total: u64,
    pub(super) told: u64,
}

impl Progress<'_> {
    fn add(&mut self, n: u64) {
        self.done += n;
        if self.done - self.told >= PROGRESS_EVERY {
            self.told = self.done;
            (self.tell)(self.done, self.total.max(self.done));
        }
    }

    /// A try that failed halfway: its bytes don't count.
    fn back_to(&mut self, done: u64) {
        self.done = done;
        self.told = self.told.min(done);
    }

    pub(super) fn end(&mut self) {
        self.told = self.done;
        (self.tell)(self.done, self.total.max(self.done));
    }
}

/// How one try of a request ended badly: worth another try (a lost connection), or not.
enum Failed {
    Again,
    Stop(FontError),
}

fn stopped(cancel: &AtomicBool) -> Result<(), Failed> {
    if cancel.load(Ordering::SeqCst) {
        Err(Failed::Stop(FontError::Cancelled))
    } else {
        Ok(())
    }
}

/// `url` into `part` (made anew), at most `cap` bytes: its length and SHA-256, hashed as it is written. A lost
/// connection is tried again twice (from the start: a `.part` is never resumed), then `Offline`. A cancel is seen before
/// every request, retries included, and between chunks.
pub(super) fn fetch(
    client: &Client,
    url: &Url,
    part: &Path,
    cap: u64,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> Result<(u64, String), FontError> {
    let start = progress.done;
    let mut tries = 0;
    loop {
        match fetch_once(client, url, part, cap, progress, cancel) {
            Ok(got) => return Ok(got),
            Err(Failed::Stop(e)) => return Err(e),
            Err(Failed::Again) if tries < RETRIES => {
                tries += 1;
                progress.back_to(start);
            }
            Err(Failed::Again) => return Err(FontError::Offline),
        }
    }
}

fn fetch_once(
    client: &Client,
    url: &Url,
    part: &Path,
    cap: u64,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> Result<(u64, String), Failed> {
    stopped(cancel)?;
    let mut reply = client.get(url.clone()).send().map_err(|_| Failed::Again)?;
    let status = reply.status();
    if !status.is_success() {
        return Err(Failed::Stop(FontError::Http(status.as_u16())));
    }
    // Read before the body: reqwest's length is what's left of it.
    let expected = reply.content_length();
    if expected.is_some_and(|n| n > cap) {
        return Err(Failed::Stop(FontError::TooBig));
    }
    let mut out = Writer::create(part).map_err(|e| Failed::Stop(FontError::disk(&e)))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        stopped(cancel)?;
        let n = match reply.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Failed::Again),
        };
        total += n as u64;
        if total > cap {
            return Err(Failed::Stop(FontError::TooBig));
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])
            .map_err(|e| Failed::Stop(FontError::disk(&e)))?;
        progress.add(n as u64);
    }
    // A reply cut short (its length said more than came) is a lost connection.
    if expected.is_some_and(|n| n != total) {
        return Err(Failed::Again);
    }
    out.finish()
        .map_err(|e| Failed::Stop(FontError::disk(&e)))?;
    Ok((total, hex(&hasher.finalize())))
}

/// A small file (a preview: at most 1 MB) into memory, within `timeout`, tried `1 + retries` times.
pub(super) fn fetch_small(
    client: &Client,
    url: &Url,
    cancel: &AtomicBool,
    timeout: Duration,
    retries: u32,
) -> Result<Vec<u8>, FontError> {
    let mut tries = 0;
    loop {
        let got = stopped(cancel).and_then(|()| {
            let reply = client
                .get(url.clone())
                .timeout(timeout)
                .send()
                .map_err(|_| Failed::Again)?;
            if !reply.status().is_success() {
                return Err(Failed::Stop(FontError::Http(reply.status().as_u16())));
            }
            let mut bytes = Vec::new();
            reply
                .take(MAX_SMALL_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| Failed::Again)?;
            if bytes.len() as u64 > MAX_SMALL_BYTES {
                return Err(Failed::Stop(FontError::TooBig));
            }
            Ok(bytes)
        });
        match got {
            Ok(bytes) => return Ok(bytes),
            Err(Failed::Stop(e)) => return Err(e),
            Err(Failed::Again) if tries < retries => tries += 1,
            Err(Failed::Again) => return Err(FontError::Offline),
        }
    }
}

/// The `.part` writer: a file synced before it is renamed. Under `test-hosts` it can answer a full disk part-way
/// (`test_disk_full_after`).
struct Writer {
    file: File,
    #[cfg(feature = "test-hosts")]
    full_after: Option<u64>,
    #[cfg(feature = "test-hosts")]
    written: u64,
}

impl Writer {
    fn create(path: &Path) -> io::Result<Writer> {
        let file = File::create(path)?;
        #[cfg(feature = "test-hosts")]
        let full_after = disk_full_for(path);
        Ok(Writer {
            file,
            #[cfg(feature = "test-hosts")]
            full_after,
            #[cfg(feature = "test-hosts")]
            written: 0,
        })
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        #[cfg(feature = "test-hosts")]
        {
            self.written += bytes.len() as u64;
            if self.full_after.is_some_and(|n| self.written > n) {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "no space left (test)",
                ));
            }
        }
        self.file.write_all(bytes)
    }

    fn finish(self) -> io::Result<()> {
        durable::sync(&self.file, Durability::Ordered)
    }
}

#[cfg(feature = "test-hosts")]
thread_local! {
    /// `test_disk_full_after`'s file name prefix and byte count, for this thread's next such file.
    static DISK_FULL: std::cell::RefCell<Option<(String, u64)>> = const { std::cell::RefCell::new(None) };
}

/// Tests: the next `.part` on this thread whose name starts with `name` answers a full disk past `bytes` (once).
#[cfg(feature = "test-hosts")]
pub fn test_disk_full_after(name: &str, bytes: u64) {
    DISK_FULL.with(|d| *d.borrow_mut() = Some((name.to_owned(), bytes)));
}

#[cfg(feature = "test-hosts")]
fn disk_full_for(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    DISK_FULL.with(|d| {
        let mut d = d.borrow_mut();
        match d.as_ref() {
            Some((prefix, n)) if name.starts_with(prefix.as_str()) => {
                let n = *n;
                *d = None;
                Some(n)
            }
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tail_never_leaves_googles_folder() {
        let base = Url::parse(FILES).unwrap();
        assert_eq!(
            url(&base, "inter/v20/abc.ttf").unwrap().as_str(),
            "https://fonts.gstatic.com/s/inter/v20/abc.ttf"
        );
        for tail in [
            "../evil.ttf",
            "//evil.example/x.ttf",
            "https://evil.example/x.ttf",
            "/x.ttf",
            "a/../../x.ttf",
            "x.ttf?y=1",
            "%2e%2e/x.ttf",
            "a@b/x.ttf",
            "",
        ] {
            assert!(url(&base, tail).is_err(), "{tail}");
        }
    }
}
