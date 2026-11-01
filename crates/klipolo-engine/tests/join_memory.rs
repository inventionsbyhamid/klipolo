//! The join's memory, in a test binary of its own: it measures this whole process's footprint, so no other test may run
//! beside it (in `export.rs` it shared the process with about 30 tests running in parallel).
mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use klipolo_engine::export::finalize::join_files;
use klipolo_engine::export::quality::Container;
use klipolo_engine::export::shared::FrameRate;
use support::fixture;

/// The join holds no packet once it is written, so its memory stays flat however long the file.
/// Ten copies of a 35 MB, 130 s file (347 MB of video, with its sound) are joined while this process's footprint is
/// sampled. Before the fix every packet read leaked (FFmpeg 9's `av_read_frame` resets a reused packet without
/// unreferencing it), and the footprint grew by the file's size.
#[test]
fn a_long_join_keeps_its_memory_flat() {
    const LIMIT: u64 = 150 << 20;
    let source = fixture("scan-130s-720p.mp4");
    let files: Vec<(PathBuf, u64)> = (0..10).map(|i| (source.clone(), i * 3900)).collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("joined.mp4");
    let footprint = || {
        klipolo_platform::process::memory(std::process::id())
            .expect("our own memory")
            .footprint
    };
    let before = footprint();
    let (peak, on) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicBool::new(true)),
    );
    let sampler = std::thread::spawn({
        let (peak, on) = (peak.clone(), on.clone());
        move || {
            while on.load(Ordering::Relaxed) {
                peak.fetch_max(footprint() as usize, Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    });
    let bytes = join_files(
        &files,
        Some(&source),
        &path,
        Container::Mp4,
        FrameRate::whole(30),
        10 * 3900,
        &mut || {},
    );
    on.store(false, Ordering::Relaxed);
    sampler.join().unwrap();
    let bytes = bytes.unwrap();
    let (peak, after) = (peak.load(Ordering::Relaxed) as u64, footprint());
    let mb = |b: u64| b >> 20;
    eprintln!(
        "join of {} MB: footprint {} MB before, peak {} MB, {} MB after",
        mb(bytes),
        mb(before),
        mb(peak),
        mb(after)
    );
    assert!(bytes > 300 << 20, "{bytes} bytes");
    assert!(
        peak.max(after).saturating_sub(before) < LIMIT,
        "grew {} MB joining {} MB",
        mb(peak.max(after).saturating_sub(before)),
        mb(bytes)
    );
}
