//! The 56-byte little-endian trailer after each frame's pixels. Pixels start at offset 0, so the webview
//! uploads them without copying. Mirrored by `apps/desktop/src/preview/frameTrailer.ts`.

use std::time::{SystemTime, UNIX_EPOCH};

pub const TRAILER_LEN: usize = 56;
/// "KLF2" read as a little-endian u32 (version 2 added the pixel format and the due time).
pub const MAGIC: u32 = u32::from_le_bytes(*b"KLF2");
/// RGBA, 8 bits per channel: `stride × height` bytes.
pub const FORMAT_RGBA8: u16 = 0;
/// NV12 (BT.709 limited range): a `stride × height` Y plane, then a `stride × height / 2` interleaved UV plane.
pub const FORMAT_NV12: u16 = 1;
/// The frame belongs to playback (not a still after a seek or an edit).
pub const FLAG_PLAYBACK: u16 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameMeta {
    pub width: u32,
    pub height: u32,
    /// Bytes per row: at least `width × 4` for RGBA (wgpu readback rows are 256-byte aligned), at least `width`
    /// for NV12 (both planes share it).
    pub stride: u32,
    pub format: u16,
    pub flags: u16,
    /// The last UI seek sequence applied before this render.
    pub seq: u32,
    /// Engine session; it changes whenever the open project changes.
    pub session: u32,
    /// Project revision rendered (low 32 bits).
    pub revision: u32,
    /// Timeline time shown, in µs.
    pub time_us: i64,
    /// Send time in ms since the Unix epoch (diagnostics only).
    pub sent_ms: f64,
    /// When the frame should be on screen, in ms since the Unix epoch: the moment its sound is heard (playback
    /// frames are drawn ahead). 0: show it as soon as it arrives (stills).
    pub due_ms: f64,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TrailerError {
    #[error("the frame is shorter than its trailer")]
    TooShort,
    #[error("the frame trailer has the wrong magic number")]
    BadMagic,
    #[error("the frame's stride or size doesn't fit the buffer")]
    BadSize,
    #[error("the frame has an unknown pixel format")]
    BadFormat,
}

/// Bytes of pixels a frame of this format and size holds.
pub fn pixel_bytes(format: u16, stride: u32, height: u32) -> u64 {
    let rows = u64::from(stride) * u64::from(height);
    if format == FORMAT_NV12 {
        rows + u64::from(stride) * u64::from(height.div_ceil(2))
    } else {
        rows
    }
}

/// Appends the trailer in place, after the pixels already in `buf` (no copy of the pixels).
pub fn append_trailer(buf: &mut Vec<u8>, m: &FrameMeta) {
    buf.reserve_exact(TRAILER_LEN);
    buf.extend_from_slice(&MAGIC.to_le_bytes());
    buf.extend_from_slice(&m.width.to_le_bytes());
    buf.extend_from_slice(&m.height.to_le_bytes());
    buf.extend_from_slice(&m.stride.to_le_bytes());
    buf.extend_from_slice(&m.format.to_le_bytes());
    buf.extend_from_slice(&m.flags.to_le_bytes());
    buf.extend_from_slice(&m.seq.to_le_bytes());
    buf.extend_from_slice(&m.session.to_le_bytes());
    buf.extend_from_slice(&m.revision.to_le_bytes());
    buf.extend_from_slice(&m.time_us.to_le_bytes());
    buf.extend_from_slice(&m.sent_ms.to_le_bytes());
    buf.extend_from_slice(&m.due_ms.to_le_bytes());
}

pub fn read_trailer(buf: &[u8]) -> Result<FrameMeta, TrailerError> {
    let len = buf.len();
    if len < TRAILER_LEN {
        return Err(TrailerError::TooShort);
    }
    let t = &buf[len - TRAILER_LEN..];
    let u16_at = |o: usize| u16::from_le_bytes([t[o], t[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(t[o..o + 4].try_into().expect("4 bytes"));
    let f64_at = |o: usize| f64::from_le_bytes(t[o..o + 8].try_into().expect("8 bytes"));
    if u32_at(0) != MAGIC {
        return Err(TrailerError::BadMagic);
    }
    let format = u16_at(16);
    if format != FORMAT_RGBA8 && format != FORMAT_NV12 {
        return Err(TrailerError::BadFormat);
    }
    let m = FrameMeta {
        width: u32_at(4),
        height: u32_at(8),
        stride: u32_at(12),
        format,
        flags: u16_at(18),
        seq: u32_at(20),
        session: u32_at(24),
        revision: u32_at(28),
        time_us: i64::from_le_bytes(t[32..40].try_into().expect("8 bytes")),
        sent_ms: f64_at(40),
        due_ms: f64_at(48),
    };
    let row = if format == FORMAT_NV12 { 1 } else { 4 };
    if u64::from(m.stride) < u64::from(m.width) * row
        || pixel_bytes(format, m.stride, m.height) > (len - TRAILER_LEN) as u64
    {
        return Err(TrailerError::BadSize);
    }
    Ok(m)
}

pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}
