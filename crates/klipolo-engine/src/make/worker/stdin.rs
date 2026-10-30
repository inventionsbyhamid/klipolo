//! The worker's stdin as `AsyncRead` (the engine builds tokio without `io-std`): a thread of its own reads it in
//! chunks into a short channel, so at most a few chunks wait in memory. When stdin ends or fails it says so in
//! `closed`, which the watchdog reads (`watchdog`).

use std::io::{self, Read};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc;

const CHUNK: usize = 64 * 1024;

pub(super) struct Stdin {
    chunks: mpsc::Receiver<io::Result<Vec<u8>>>,
    chunk: Vec<u8>,
    at: usize,
}

impl Stdin {
    /// Starts the reading thread (inside the sandbox, after the profile is applied); `closed` is set once stdin ends or
    /// fails.
    pub fn spawn(closed: Arc<AtomicBool>) -> io::Result<Stdin> {
        let (send, chunks) = mpsc::channel(4);
        std::thread::Builder::new()
            .name("render-worker-stdin".into())
            .spawn(move || {
                let mut stdin = io::stdin().lock();
                let ended = || closed.store(true, Ordering::SeqCst);
                loop {
                    let mut chunk = vec![0; CHUNK];
                    let read = match stdin.read(&mut chunk) {
                        Ok(0) => return ended(),
                        Ok(n) => n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            ended();
                            let _ = send.blocking_send(Err(e));
                            return;
                        }
                    };
                    chunk.truncate(read);
                    if send.blocking_send(Ok(chunk)).is_err() {
                        return;
                    }
                }
            })?;
        Ok(Stdin {
            chunks,
            chunk: Vec::new(),
            at: 0,
        })
    }
}

impl AsyncRead for Stdin {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.at >= self.chunk.len() {
            match ready!(self.chunks.poll_recv(cx)) {
                // Stdin ended: the parent is done.
                None => return Poll::Ready(Ok(())),
                Some(Err(e)) => return Poll::Ready(Err(e)),
                Some(Ok(chunk)) => {
                    self.chunk = chunk;
                    self.at = 0;
                }
            }
        }
        let n = (self.chunk.len() - self.at).min(buf.remaining());
        let at = self.at;
        buf.put_slice(&self.chunk[at..at + n]);
        self.at += n;
        Poll::Ready(Ok(()))
    }
}
