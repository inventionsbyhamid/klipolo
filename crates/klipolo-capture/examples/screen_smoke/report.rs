//! The smoke run's numbers (never content): the take's length and notes, each run's start, length, bitrate, drift and
//! levels, a video run's frames, keyframes and intervals, and the cursor file's counts, burn-in and press times.

use std::path::Path;

use klipolo_capture::backend::PresetId;
use klipolo_capture::fmp4::read::walk;
use klipolo_capture::manifest::Manifest;
use klipolo_capture::media::StreamInfo;
use klipolo_capture::preset::native_preset;
use klipolo_capture::session::SessionDir;

/// Numbers only, never content.
pub fn report(dir: &SessionDir, preset: PresetId) {
    let Ok(bytes) = std::fs::read(dir.manifest()) else {
        return println!("no manifest");
    };
    let m: Manifest = serde_json::from_slice(&bytes).expect("a manifest");
    println!(
        "take {:.2} s, notes {:?}",
        m.duration_us as f64 / 1e6,
        (m.notes.iter()).map(|n| &n.code).collect::<Vec<_>>()
    );
    for s in &m.sources {
        for r in &s.runs {
            let path = dir.path(&r.file);
            let size = std::fs::metadata(&path).map_or(0, |m| m.len());
            let secs = r.duration_us as f64 / 1e6;
            print!(
                "{} {}: starts at {:.1} ms, {:.2} s, {:.2} Mb/s, drift {:.1} ppm",
                s.id,
                r.file,
                r.offset_us as f64 / 1e3,
                secs,
                size as f64 * 8.0 / secs.max(1e-3) / 1e6,
                r.drift_ppm
            );
            if r.approximate {
                print!(", placed approximately (no drift line)");
            }
            if let Some(peaks) = &r.channel_peaks {
                let db: Vec<String> = peaks
                    .iter()
                    .map(|p| format!("{:.1}", 20.0 * p.max(1e-9).log10()))
                    .collect();
                print!(", peaks dBFS {db:?}");
            }
            println!();
            if r.file.ends_with(".mp4") {
                video(&path, preset);
            }
        }
    }
    if let Ok(bytes) = std::fs::read(dir.cursor()) {
        let limits = klipolo_cursor::Limits::for_file_len(bytes.len() as u64);
        let track = klipolo_cursor::CursorTrack::read(&bytes[..], limits);
        let (from, to) = track.span().unwrap_or((0, 0));
        let span = (to - from) as f64 / 1e6;
        let inside = track
            .samples(from, to + 1)
            .filter(|&(_, x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y))
            .count();
        println!(
            "cursor: {} positions over {:.2} s ({:.1} a second), {} inside the picture, {} presses, {} skipped",
            track.len(),
            span,
            track.len() as f64 / span.max(1e-3),
            inside,
            track.presses().len(),
            track.skipped()
        );
        // Separate, then burned in: the first time it isn't (a binary search; None while it stays separate).
        let (mut lo, mut hi) = (from, to);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            (lo, hi) = if track.separate_at(mid) {
                (mid + 1, hi)
            } else {
                (lo, mid)
            };
        }
        let burned_from = (!track.separate_at(to) && track.separate_at(from)).then_some(lo);
        println!(
            "cursor: header {:?}, clicks weakest {:?}, {} shapes, burned in from {:?} µs",
            track.header(),
            track.clicks_weakest(),
            track.shapes().len(),
            burned_from
        );
        // The drift check (with the source's "first and last downs" log line): the presses' file times.
        let (first, last) = (track.presses().first(), track.presses().last());
        println!(
            "cursor: first press down at {:?} µs, last at {:?} µs",
            first.map(|p| p.down),
            last.map(|p| p.down)
        );
    }
}

/// The video run's frames, keyframes and intervals from its own sample table.
fn video(path: &Path, preset: PresetId) {
    let file = std::fs::File::open(path).expect("the run");
    let len = file.metadata().map_or(0, |m| m.len());
    let w = walk(&mut &file, len).expect("a walk");
    let samples: Vec<_> = w.index.fragments.iter().flat_map(|f| &f.samples).collect();
    let keys = samples.iter().filter(|s| s.key).count();
    let Some(info) = w.head.as_ref().map(|h| &h.info) else {
        return println!("  no head");
    };
    let target = native_preset(preset, info.codec, (info.width, info.height));
    let tick = 1e3 / f64::from(klipolo_capture::fmp4::TIMESCALE);
    let ms: Vec<f64> = samples
        .iter()
        .map(|s| f64::from(s.duration) * tick)
        .collect();
    let frame_ms = 1e3 / f64::from(target.fps);
    let within = ms.iter().filter(|&&d| d <= frame_ms * 1.5).count();
    let longest = ms.iter().copied().fold(0.0, f64::max);
    let StreamInfo {
        codec,
        width,
        height,
        ..
    } = info;
    println!(
        "  {codec:?} {width}x{height}, remuxed {}, {} frames, {keys} keyframes, intervals within 1.5x of {frame_ms:.1} ms: {:.1} %, longest {longest:.0} ms, preset bitrate {} Mb/s",
        w.remuxed,
        samples.len(),
        100.0 * within as f64 / ms.len().max(1) as f64,
        target.bitrate_bps / 1_000_000
    );
}
