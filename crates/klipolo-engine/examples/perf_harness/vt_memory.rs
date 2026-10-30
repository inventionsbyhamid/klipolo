//! `perf_harness vt-memory <file>...`: VideoToolbox memory with 1, 2, 4 and 6 preview decoders on one file, each a
//! reader of its own at its own time, as the preview pool opens them (main role: a 1920×1080 output
//! box and a 96 MiB frame cache). Over 3 s of 30 fps frames per decoder it samples this process's footprint and every
//! VTDecoderXPCService's (VideoToolbox decodes out of process), and reports the peaks over the idle baseline.

use std::path::PathBuf;
use std::time::Duration;

use klipolo_engine::render::PREVIEW_TUNING;
use klipolo_engine::render::probe::probe_media;
use klipolo_engine::vendor::cap::rendering::decoder::spawn_decoder;

use super::measure::{mb, me, vt_footprint};

/// The thread names the decoders get (Cap's take `&'static str`).
const NAMES: [&str; 6] = [
    "vt-mem-0", "vt-mem-1", "vt-mem-2", "vt-mem-3", "vt-mem-4", "vt-mem-5",
];

pub async fn run(files: &[String]) {
    println!(
        "file | decoders | ours peak MB (+ over idle) | VT services peak MB (+ over idle) | together + MB | each reader's path"
    );
    for file in files {
        let path = PathBuf::from(file);
        let info = probe_media(&path).expect("a video");
        let fps = if info.fps > 0.0 { info.fps } else { 30.0 };
        for decoders in [1usize, 2, 4, 6] {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let (idle_me, idle_vt) = (me().footprint, vt_footprint());
            let mut handles = Vec::new();
            for name in NAMES.iter().take(decoders) {
                handles.push(
                    spawn_decoder(name, path.clone(), fps, 0.0, false, PREVIEW_TUNING)
                        .await
                        .expect("a decoder"),
                );
            }
            let (mut peak_me, mut peak_vt) = (idle_me, idle_vt);
            // Each decoder plays 3 s from its own start, spread over the file (a cut-heavy timeline's lanes).
            let span = (info.duration - 3.5).max(0.0);
            for frame in 0..90 {
                for (i, h) in handles.iter().enumerate() {
                    let start = span * i as f64 / decoders.max(1) as f64;
                    let t = start + (f64::from(frame) + 0.5) / fps;
                    let _ = h.get_frame(t as f32).await;
                }
                if frame % 5 == 0 {
                    peak_me = peak_me.max(me().footprint);
                    peak_vt = peak_vt.max(vt_footprint());
                }
            }
            // Which path each reader ended on (do 4–6 Main 10 readers fall back to FFmpeg's software
            // decoder once VideoToolbox takes no more sessions?), and why it fell back.
            let paths: Vec<String> = handles
                .iter()
                .map(|h| match h.fallback_reason() {
                    Some(why) => format!("{} ({why})", h.decoder_type()),
                    None => h.decoder_type().to_string(),
                })
                .collect();
            println!(
                "{} | {decoders} | {:.0} (+{:.0}) | {:.0} (+{:.0}) | +{:.0} | {}",
                path.file_name().unwrap().to_string_lossy(),
                mb(peak_me),
                mb(peak_me.saturating_sub(idle_me)),
                mb(peak_vt),
                mb(peak_vt.saturating_sub(idle_vt)),
                mb(peak_me.saturating_sub(idle_me) + peak_vt.saturating_sub(idle_vt)),
                paths.join(", "),
            );
            drop(handles);
        }
    }
}
