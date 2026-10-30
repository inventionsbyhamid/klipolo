//! What the stalled-decoder tests share (`trial` builds): the film fixture, its frames' luma and times, a
//! source on it, and the guard that clears a test's hooks.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use klipolo_engine::decode_wait::hooks::{self, ReadStall, SlowSoftware, Stall};
use klipolo_engine::plan::SourceRef;
use klipolo_engine::vendor::cap::rendering::decoder::DecodedFrame;

use super::fixture;

pub const FILM: &str = "film-1080p-gop100-12s.mp4";
pub const FILM_FPS: f64 = 24_000.0 / 1_001.0;
/// The frame an export on the test Mac stalled before (8.78 s), and the keyframe before it.
pub const STALL_AT: u32 = 210;
pub const KEYFRAME: u32 = 200;

/// The tests share process-wide counts and export's wait: they take turns.
pub static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The film linked (or, across volumes, copied) into `dir` as `name`: each test's hooks are its own file's.
pub fn film(dir: &Path, name: &str) -> Arc<str> {
    let path = dir.join(name);
    std::fs::hard_link(fixture(FILM), &path)
        .or_else(|_| std::fs::copy(fixture(FILM), &path).map(drop))
        .unwrap();
    path.display().to_string().into()
}

/// The film's luma at frame `n` (the fixture's rise and fall in steps of 8).
pub fn film_luma(n: u32) -> u8 {
    let t = n % 50;
    16 + 8 * if t < 25 { t } else { 49 - t } as u8
}

/// The middle of frame `n`, as a source time.
pub fn at_frame(n: u32) -> i64 {
    ((f64::from(n) + 0.5) / FILM_FPS * 1e6) as i64
}

pub fn seconds(n: u32) -> f64 {
    f64::from(n) / FILM_FPS
}

/// Clears a file's hooks and export's wait when a test ends, passed or not, so no reader stays blocked.
pub struct Hooked(pub Vec<Arc<str>>);

impl Drop for Hooked {
    fn drop(&mut self) {
        for file in &self.0 {
            hooks::clear(file);
        }
        hooks::set_export_wait(None);
        hooks::set_reopen_quiet(None);
    }
}

pub fn source(file: &Arc<str>, frame: u32) -> SourceRef {
    SourceRef {
        asset_id: file.clone(),
        path: file.clone(),
        original: file.clone(),
        time_us: at_frame(frame),
        fps: FILM_FPS,
        rotation: Some(0),
        width: 1920,
        height: 1080,
    }
}

/// The luma in the middle of a decoded frame.
pub fn middle_luma(frame: &DecodedFrame) -> u8 {
    let (w, h, stride) = (frame.width(), frame.height(), frame.y_stride());
    frame.y_plane().expect("a YUV frame")[(h / 2 * stride + w / 2) as usize]
}

/// A software decoder that stops at frame 210 (the first `decoders` opened on the file from now do).
pub fn stops_at_210(decoders: usize) -> SlowSoftware {
    SlowSoftware {
        decoders: 0..decoders,
        from_s: seconds(STALL_AT),
        per_frame: Duration::from_secs(600),
        frames: u32::MAX,
    }
}

/// Readers `readers` (made on the file from now) stall before frame 210 for `stall`.
pub fn at_210(readers: std::ops::Range<usize>, stall: Stall) -> ReadStall {
    ReadStall {
        readers,
        at_frame: STALL_AT,
        stall,
    }
}
