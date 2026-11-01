//! VideoToolbox on this Mac with generated frames only: nothing is captured, so these run in the Mac's test
//! command. The pinned encoder's packets become a file Klipolo's FFmpeg decodes with a keyframe every second; the
//! IOSurface pool drops and counts rather than block; VTDecompressionSession answers the read-back. The probe and the
//! cost measurement are run by hand (`--ignored`).
#![cfg(all(target_os = "macos", feature = "real-capture"))]

mod common;

use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use klipolo_capture::fmp4::Fmp4Writer;
use klipolo_capture::media::{Codec, EncodedPacket};
use klipolo_capture::pipe::PipeError;
use klipolo_capture::platform::macos::{
    EncoderSpec, SurfacePool, VtEncoder, decodes, generated_nv12, hardware_encoders,
};

/// `frames` generated frames through this Mac's first hardware encoder of `codec`; Err: VideoToolbox refused it.
fn encode(codec: Codec, frames: u64) -> Result<(VtEncoder, Vec<EncodedPacket>, String), PipeError> {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let enc_id = hardware_encoders()
        .into_iter()
        .find(|e| e.codec == codec)
        .expect("a hardware encoder")
        .id;
    let (tx, rx) = sync_channel(256);
    let (ev, _) = sync_channel(16);
    let spec = EncoderSpec {
        encoder_id: enc_id.clone(),
        codec,
        width: 640,
        height: 360,
        fps: 30,
        bitrate_bps: 4_000_000,
    };
    let mut vt = VtEncoder::new(&spec, tx, ev)?;
    let pictures = generated_nv12(640, 360, 4);
    for i in 0..frames {
        vt.encode(&pictures[(i % 4) as usize], i * 1_000_000_000 / 30, false)
            .unwrap();
    }
    vt.finish();
    let packets = rx.try_iter().collect();
    Ok((vt, packets, enc_id))
}

#[test]
fn generated_frames_encode_to_a_file_ffmpeg_decodes_with_a_keyframe_every_second() {
    let (vt, packets, enc_id) = encode(Codec::H264, 90).unwrap();
    assert_eq!(
        vt.actual_encoder_id().as_deref(),
        Some(enc_id.as_str()),
        "pinned"
    );
    assert_eq!(packets.len(), 90);
    assert_eq!(
        packets.iter().filter(|p| p.keyframe).count(),
        3,
        "one keyframe a second: MaxKeyFrameInterval = fps"
    );
    let mut w = Fmp4Writer::new(&vt.info().unwrap());
    let mut bytes = w.head();
    for p in &packets {
        if let Some(f) = w.push((p.host_ns / 1_000) as i64, p) {
            bytes.extend(f.bytes);
        }
    }
    if let Some(f) = w.flush() {
        bytes.extend(f.bytes);
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("t.mp4"), &bytes).unwrap();
    assert_eq!(common::decode::decode_all(&dir.path().join("t.mp4")).0, 90);
}

#[test]
fn a_full_pool_drops_and_counts_instead_of_blocking() {
    let mut pool = SurfacePool::new(320, 180, 2).unwrap();
    let frames = generated_nv12(320, 180, 1);
    let a = pool.copy(&frames[0]).unwrap();
    let b = pool.copy(&frames[0]).unwrap();
    assert!(pool.copy(&frames[0]).is_none());
    assert_eq!(pool.dropped(), 1);
    drop((a, b));
    assert!(
        pool.copy(&frames[0]).is_some(),
        "returned buffers are reused"
    );
}

/// The 10 s read-back (`Backend::decodes`): the run's first keyframe decodes from its stream description; bytes that
/// aren't a picture don't.
#[test]
fn the_read_back_decodes_a_keyframe_from_the_stream_s_description() {
    for codec in [Codec::H264, Codec::Hevc] {
        if !hardware_encoders().iter().any(|e| e.codec == codec) {
            continue;
        }
        // HEVC where this Mac's encoder takes a session (on 15.7.5 it refused, system-wide).
        let (vt, packets, _) = match encode(codec, 3) {
            Err(PipeError::Encoder(code)) if codec == Codec::Hevc => {
                eprintln!("HEVC not checked: this Mac's HEVC encoder refused a session ({code})");
                continue;
            }
            made => made.unwrap(),
        };
        let info = vt.info().unwrap();
        let key = packets.iter().find(|p| p.keyframe).unwrap();
        assert_eq!(decodes(&info, &key.data), Some(true), "{codec:?}");
        // A well-formed sample whose one NAL unit is the keyframe's first 8 bytes: a slice header, no picture.
        let mut torn = vec![0, 0, 0, 8];
        torn.extend_from_slice(&key.data[4..12]);
        assert_eq!(
            decodes(&info, &torn),
            Some(false),
            "{codec:?}: a torn keyframe"
        );
    }
}

/// Run by hand on a Mac after a Mac build (quietly): the probe's real numbers and its time.
#[test]
#[ignore]
fn the_probe_finishes_within_five_seconds() {
    let report = klipolo_capture::platform::backend_for_probe().probe(Duration::from_secs(5));
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    assert!(report.took_ms <= 5_000);
}

/// What recording costs this Mac per frame, by hand: generated 1080p60 frames copied through the pool on the GPU and
/// encoded at Smooth's bitrate on each hardware encoder for 5 s, paced. Prints the process's CPU time per second of
/// video (the worker's share; VideoToolbox's own service is measured apart with `ps`), the copy's cost, drops and the
/// encode latency.
#[test]
#[ignore]
fn encode_cost_at_1080p60() {
    use klipolo_capture::backend::PresetId;
    use klipolo_capture::preset::preset;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    for e in hardware_encoders() {
        let p = preset(PresetId::Smooth, e.codec);
        let (tx, rx) = sync_channel(256);
        let (ev, _) = sync_channel(16);
        let spec = EncoderSpec {
            encoder_id: e.id.clone(),
            codec: e.codec,
            width: p.width,
            height: p.height,
            fps: p.fps,
            bitrate_bps: p.bitrate_bps,
        };
        let mut vt = match VtEncoder::new(&spec, tx, ev) {
            Ok(vt) => vt,
            Err(refused) => {
                println!("{}: refused a session ({refused:?})", e.id);
                continue;
            }
        };
        let mut pool = SurfacePool::for_preset(p.width, p.height, p.fps).unwrap();
        let pictures = generated_nv12(p.width, p.height, 4);
        let frames = u64::from(p.fps) * 5;
        let (cpu0, start) = (cpu_s(), Instant::now());
        let mut copy_ms: Vec<f64> = Vec::with_capacity(frames as usize);
        let (mut bytes, mut keys, mut got) = (0usize, 0usize, 0usize);
        for i in 0..frames {
            let due = start + Duration::from_nanos(i * 1_000_000_000 / u64::from(p.fps));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let t = Instant::now();
            let Some(buf) = pool.copy(&pictures[(i % 4) as usize]) else {
                continue;
            };
            copy_ms.push(t.elapsed().as_secs_f64() * 1e3);
            vt.encode(&buf, i * 1_000_000_000 / u64::from(p.fps), false)
                .unwrap();
            for packet in rx.try_iter() {
                (bytes, keys, got) = (
                    bytes + packet.data.len(),
                    keys + usize::from(packet.keyframe),
                    got + 1,
                );
            }
        }
        vt.finish();
        for packet in rx.try_iter() {
            (bytes, keys, got) = (
                bytes + packet.data.len(),
                keys + usize::from(packet.keyframe),
                got + 1,
            );
        }
        let wall = start.elapsed().as_secs_f64();
        copy_ms.sort_by(f64::total_cmp);
        let at = |q: f64| copy_ms[((copy_ms.len() as f64 - 1.0) * q) as usize];
        println!(
            "{} ({:?}) {}x{}@{}: {got}/{frames} packets, {keys} keyframes, {:.1} Mb/s; worker CPU {:.3} core; pool copy \
             p50 {:.2} ms p99 {:.2} ms; pool {} frames, dropped {}; encode p99 {:.1} ms; wall {wall:.2} s",
            e.id,
            e.codec,
            p.width,
            p.height,
            p.fps,
            bytes as f64 * 8.0 / 5.0 / 1e6,
            (cpu_s() - cpu0) / wall,
            at(0.5),
            at(0.99),
            pool.frames(),
            pool.dropped(),
            vt.latency_p99_ms(),
        );
    }
}

/// This process's user and system CPU time so far, in seconds.
fn cpu_s() -> f64 {
    // SAFETY: getrusage fills the struct it is given.
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let s = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    s(usage.ru_utime) + s(usage.ru_stime)
}
