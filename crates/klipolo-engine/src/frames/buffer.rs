//! Reused frame buffers. A 720p RGBA frame is 3.7 MB: allocating one per frame costs about 900 first-touch page
//! faults and 110 MB/s of `vm_allocate`/free at 30 fps. Frames are copied into buffers from a
//! small pool instead, and handed to the socket as `Bytes::from_owner`, so the buffer goes back to the pool when the
//! last copy of the frame (the socket's send, the newest-frame slot) lets go of it.

use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;

use super::trailer::TRAILER_LEN;

/// Buffers kept for reuse: one being filled, the newest published frame and one being sent, plus a spare.
const KEEP: usize = 4;

/// A pool of frame buffers, shared by everything that makes frames for one engine.
#[derive(Clone, Default)]
pub struct FramePool {
    free: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl FramePool {
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty buffer that holds `pixels` bytes plus the frame trailer without growing.
    pub fn take(&self, pixels: usize) -> FrameBytes {
        let want = pixels + TRAILER_LEN;
        let reused = {
            let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
            // Prefer one that is already big enough (the size only changes when the preview is resized).
            match free.iter().position(|b| b.capacity() >= want) {
                Some(i) => Some(free.swap_remove(i)),
                None => free.pop(),
            }
        };
        let mut buf = reused.unwrap_or_default();
        buf.clear();
        buf.reserve_exact(want);
        FrameBytes {
            buf,
            pool: Some(self.clone()),
        }
    }

    /// Buffers waiting to be reused (tests).
    pub fn idle(&self) -> usize {
        self.free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn give_back(&self, buf: Vec<u8>) {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        if free.len() < KEEP {
            free.push(buf);
        }
    }
}

/// Frame pixels (then the trailer) in a pooled buffer; it returns to its pool when dropped.
pub struct FrameBytes {
    buf: Vec<u8>,
    pool: Option<FramePool>,
}

impl FrameBytes {
    /// A buffer that belongs to no pool (tests, the blank backend's first frames).
    pub fn unpooled(buf: Vec<u8>) -> Self {
        Self { buf, pool: None }
    }

    /// The buffer itself, to fill or to append the trailer to.
    pub fn vec_mut(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }

    /// The frame as `Bytes` without copying; the buffer goes back to the pool when the last clone is dropped.
    pub fn into_bytes(self) -> Bytes {
        Bytes::from_owner(self)
    }
}

impl std::ops::Deref for FrameBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.buf
    }
}

impl AsRef<[u8]> for FrameBytes {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

impl Drop for FrameBytes {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool.give_back(std::mem::take(&mut self.buf));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_frames_come_back_to_the_pool() {
        let pool = FramePool::new();
        let mut frame = pool.take(1024);
        frame.vec_mut().extend_from_slice(&[7; 1024]);
        let ptr = frame.as_ptr();
        let bytes = frame.into_bytes();
        let sent = bytes.clone();
        drop(bytes);
        assert_eq!(pool.idle(), 0, "still being sent");
        drop(sent);
        assert_eq!(pool.idle(), 1);
        let again = pool.take(1024);
        assert_eq!(again.buf.as_ptr(), ptr, "the same allocation, reused");
        assert!(again.is_empty() && again.buf.capacity() >= 1024 + TRAILER_LEN);
    }
}
