mod support;
use std::io::Write;
use std::path::Path;

use klipolo_engine::export::shared::{Control, FrameRate, checksum};
use klipolo_engine::media::proxy::decide::{ProxyPolicy, ProxyReason, needs_proxy, proxy_size};
use klipolo_engine::media::proxy::facts::{
    ProbeError, SourceFacts, probe_facts, probe_facts_within,
};
use klipolo_engine::media::proxy::job::{Opened, ProxyJob, Refused, Retry};
use klipolo_engine::media::proxy::run::{
    FailCode, PauseReason, ProxyEnv, RenderError, RunOutcome, Segments, run_segments,
};
use klipolo_engine::platform::gpu::GpuClass;
use klipolo_platform::power::Thermal;
use support::*;

fn facts(w: u32, h: u32, key_s: f64) -> SourceFacts {
    SourceFacts {
        width: w,
        height: h,
        rate: FrameRate::whole(30),
        codec: "h264".into(),
        ten_bit: false,
        ten_bit_hw: false,
        hdr: false,
        max_key_interval_us: secs(key_s),
        variable_rate: false,
        hw_decodes: true,
        duration_us: secs(60.0),
        start_us: 0,
        id: None,
    }
}

#[test]
fn who_needs_an_editing_copy() {
    // Pure: an H.264 encoder is assumed, so this runs the same on every OS.
    let intel = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
    let apple = ProxyPolicy::with_encoder(GpuClass::Apple, true);
    assert_eq!(
        needs_proxy(&facts(1920, 1080, 0.5), &intel),
        None,
        "Klipolo's own recordings need none"
    );
    assert_eq!(needs_proxy(&facts(1920, 1080, 1.0), &intel), None);
    assert_eq!(
        needs_proxy(&facts(1920, 1080, 2.0), &intel),
        Some(ProxyReason::LongGop)
    );
    assert_eq!(
        needs_proxy(&facts(3840, 2160, 0.5), &intel),
        Some(ProxyReason::TooLarge)
    );
    assert_eq!(needs_proxy(&facts(3840, 2160, 0.5), &apple), None);
    assert_eq!(
        needs_proxy(
            &SourceFacts {
                ten_bit: true,
                ..facts(1920, 1080, 0.5)
            },
            &apple
        ),
        Some(ProxyReason::TenBit)
    );
    assert_eq!(
        needs_proxy(
            &SourceFacts {
                hw_decodes: false,
                ..facts(1280, 720, 0.5)
            },
            &apple
        ),
        Some(ProxyReason::NoHardwareDecode)
    );
    // A phone's 10-bit HEVC comes through VideoToolbox like an 8-bit file, so its depth alone
    // gets it no copy; H.264 High 10 (above) still decodes in software.
    let main10 = SourceFacts {
        codec: "hevc".into(),
        ten_bit: true,
        ten_bit_hw: true,
        ..facts(1920, 1080, 0.5)
    };
    assert_eq!(needs_proxy(&main10, &apple), None);
    assert_eq!(needs_proxy(&main10, &intel), None);
    let uhd10 = SourceFacts {
        width: 3840,
        height: 2160,
        ..main10.clone()
    };
    // 4K Main 10's readers cost about three times 8-bit 4K's memory on an Intel Mac, so every Intel Mac copies
    // it (a discrete-GPU one too, which reads 8-bit 4K as it is); Apple silicon isn't measured yet.
    assert_eq!(needs_proxy(&uhd10, &intel), Some(ProxyReason::TenBit));
    let discrete = ProxyPolicy::with_encoder(GpuClass::Discrete, true);
    assert_eq!(needs_proxy(&uhd10, &discrete), Some(ProxyReason::TenBit));
    assert_eq!(needs_proxy(&facts(3840, 2160, 0.5), &discrete), None);
    assert_eq!(needs_proxy(&uhd10, &apple), None);
}

/// A Retina screen recording (2560×1600) is over the Intel limit. Its copy is 1280 wide: at least the
/// Intel preview's whole budget (1280×720 pixels), so the preview never shows it smaller than it would the original.
#[test]
fn a_retina_screen_recording_gets_a_1280_wide_copy_on_intel() {
    let intel = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
    let retina = facts(2560, 1600, 0.5);
    assert_eq!(needs_proxy(&retina, &intel), Some(ProxyReason::TooLarge));
    let (w, h) = proxy_size(&retina, &intel);
    assert_eq!((w, h), (1280, 800));
    assert!(w * h >= 1280 * 720);
    // Apple silicon reads it as it is.
    let apple = ProxyPolicy::with_encoder(GpuClass::Apple, true);
    assert_eq!(needs_proxy(&retina, &apple), None);
}

#[test]
fn without_an_h264_encoder_nothing_gets_a_copy() {
    // Windows and Linux today (no H.264 candidates): every file would fail, so none is queued.
    let none = ProxyPolicy::with_encoder(GpuClass::Integrated, false);
    assert_eq!(
        needs_proxy(
            &SourceFacts {
                hw_decodes: false,
                ..facts(3840, 2160, 2.0)
            },
            &none
        ),
        None
    );
}

#[test]
fn copies_are_smaller_never_larger_and_keep_their_shape() {
    let intel = ProxyPolicy::for_gpu(GpuClass::Integrated);
    assert_eq!(proxy_size(&facts(3840, 2160, 2.0), &intel), (1280, 720));
    assert_eq!(proxy_size(&facts(1080, 1920, 2.0), &intel), (720, 1280));
    assert_eq!(proxy_size(&facts(960, 540, 2.0), &intel), (960, 540));
    assert_eq!(
        proxy_size(
            &facts(1920, 1080, 2.0),
            &ProxyPolicy::for_gpu(GpuClass::Apple)
        ),
        (1920, 1080)
    );
}

#[test]
fn facts_measure_the_gop() {
    let short = probe_facts(&fixture("bars-30fps-0.5gop.mp4")).unwrap();
    assert!(
        (short.max_key_interval_us - secs(0.5)).abs() <= secs(0.05),
        "{short:?}"
    );
    let long = probe_facts(&fixture("longgop-20s-1080p.mp4")).unwrap();
    assert!(long.max_key_interval_us >= secs(9.9), "{long:?}");
    assert!(!long.ten_bit);
    // Through the seam: true on macOS (VideoToolbox), false on the Windows and Linux stubs (PS's Linux gate runs this).
    assert_eq!(
        long.hw_decodes,
        klipolo_engine::platform::decode::hw_decodes(ffmpeg::codec::Id::H264)
    );
}

/// VideoToolbox reads a phone's 10-bit 4:2:0 HEVC on the hardware path, so its depth alone
/// gets it no copy on a Mac; where the hardware path doesn't take it (Linux and Windows today), it still does.
#[test]
fn ten_bit_hevc_needs_no_copy_for_its_depth_where_videotoolbox_reads_it() {
    let f = probe_facts(&fixture("hevc10-luma-2s.mov")).unwrap();
    assert!(f.ten_bit, "{f:?}");
    assert_eq!(
        f.ten_bit_hw,
        cfg!(target_os = "macos") && f.hw_decodes,
        "{f:?}"
    );
    let reason = needs_proxy(&f, &ProxyPolicy::with_encoder(GpuClass::Apple, true));
    if f.ten_bit_hw {
        assert_ne!(reason, Some(ProxyReason::TenBit));
    } else {
        assert!(reason.is_some());
    }
}

#[test]
fn a_changed_source_gets_a_new_key_and_never_its_old_copy() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("clip.mp4");
    std::fs::copy(fixture("longgop-20s-1080p.mp4"), &src).unwrap();
    let policy = ProxyPolicy::for_gpu(GpuClass::Integrated);
    let path = src.to_string_lossy().to_string();
    let first = klipolo_engine::media::proxy::job::ProxyJob::create(
        root.path(),
        &path,
        &probe_facts(&src).unwrap(),
        &policy,
    )
    .unwrap();
    // Pretend the copy finished.
    std::fs::write(&first.out, b"copy").unwrap();
    // The file is edited: same path, new size and time.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&src)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    if let klipolo_engine::media::proxy::job::Opened::Done(p) =
        klipolo_engine::media::proxy::job::ProxyJob::open_for(root.path(), &path, &policy)
    {
        panic!("served the old copy {p:?}");
    }
}

/// A job for a stand-in source (any file: the loop only checks it is unchanged) with `segments` 10 s segments.
fn fake_job(root: &Path, segments: u64) -> ProxyJob {
    let src = root.join("source.mp4");
    std::fs::write(&src, b"a stand-in for a video").unwrap();
    let facts = SourceFacts {
        duration_us: secs(10.0 * segments as f64),
        ..facts(1920, 1080, 2.0)
    };
    let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
    let job = ProxyJob::create(
        &root.join("proxies"),
        &src.to_string_lossy(),
        &facts,
        &policy,
    )
    .unwrap();
    assert_eq!(job.segments.len() as u64, segments);
    job
}

/// Writes a small file per segment and reports its size and checksum.
#[derive(Default)]
struct FakeSegments {
    rendered: Vec<u32>,
}

impl Segments for FakeSegments {
    fn render(&mut self, job: &ProxyJob, index: u32) -> Result<(u64, u32), RenderError> {
        let path = job.path(&job.segments[index as usize].file);
        let bytes = format!("segment {index}").into_bytes();
        std::fs::write(&path, &bytes).unwrap();
        self.rendered.push(index);
        Ok((bytes.len() as u64, checksum(&path).unwrap()))
    }
}

#[test]
fn disk_full_pauses_and_keeps_finished_segments() {
    let root = tempfile::tempdir().unwrap();
    let mut job = fake_job(root.path(), 3); // three 10 s segments planned
    let env = ProxyEnv::fixed(Thermal::Nominal, |done: usize| {
        if done >= 1 { 1 << 20 } else { 100 << 30 }
    });
    let mut segments = FakeSegments::default(); // writes a small file per segment and reports its size and checksum
    let outcome = run_segments(&mut job, &env, &Control::default(), &mut segments);
    assert_eq!(outcome, RunOutcome::Paused(PauseReason::DiskFull));
    let reopened = ProxyJob::open(&job.dir).unwrap();
    assert_eq!(reopened.done_segments(), 1, "the finished segment is kept");
    // Space again: it continues from segment 1 and never renders segment 0 again.
    let outcome = run_segments(
        &mut job,
        &ProxyEnv::fixed(Thermal::Nominal, |_| 100 << 30),
        &Control::default(),
        &mut segments,
    );
    assert_eq!(outcome, RunOutcome::Finished);
    assert_eq!(segments.rendered, vec![0, 1, 2]);
}

#[test]
fn a_hot_computer_pauses_between_segments() {
    let root = tempfile::tempdir().unwrap();
    let mut job = fake_job(root.path(), 2);
    let outcome = run_segments(
        &mut job,
        &ProxyEnv::fixed(Thermal::Serious, |_| 100 << 30),
        &Control::default(),
        &mut FakeSegments::default(),
    );
    assert_eq!(outcome, RunOutcome::Paused(PauseReason::Thermal));
}

#[test]
fn a_source_that_goes_missing_fails_the_copy_by_name() {
    let root = tempfile::tempdir().unwrap();
    let mut job = fake_job(root.path(), 2);
    std::fs::remove_file(&job.source).unwrap();
    let outcome = run_segments(
        &mut job,
        &ProxyEnv::fixed(Thermal::Nominal, |_| 100 << 30),
        &Control::default(),
        &mut FakeSegments::default(),
    );
    assert_eq!(
        outcome,
        RunOutcome::Failed {
            message: "source.mp4 is missing".into(),
            missing: true,
            code: FailCode::Missing,
        }
    );
}

#[test]
fn a_slow_source_is_tried_again_later_never_stale() {
    // A probe that runs out of time (a stalled share) says so: the scheduler backs off and tries again.
    let late = probe_facts_within(&fixture("bars-30fps-0.5gop.mp4"), std::time::Duration::ZERO);
    assert_eq!(late, Err(ProbeError::TimedOut));
    let refused = Refused::from(ProbeError::TimedOut);
    assert_eq!(refused, Refused::SourceTimedOut);
    assert_eq!(refused.retry(), Retry::Backoff);
}

/// A folder this process can't write, or None (with a note) when it can anyway (root ignores permissions).
#[cfg(unix)]
fn read_only_dir(parent: &Path) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dir = parent.join("read-only");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::File::create(dir.join("probe")).is_ok() {
        eprintln!("skipped: this user can write a read-only folder");
        return None;
    }
    Some(dir)
}

#[cfg(unix)]
fn writable_again(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_full_disk_or_an_unwritable_cache_is_said_and_never_stale() {
    let full = Refused::from_io(&std::io::Error::from_raw_os_error(libc::ENOSPC));
    assert_eq!(full, Refused::DiskFull);
    assert_eq!(full.retry(), Retry::WhenSpaceFrees);
    let read_only = Refused::from_io(&std::io::Error::from_raw_os_error(libc::EROFS));
    assert!(
        matches!(read_only, Refused::CacheUnwritable(_)),
        "{read_only:?}"
    );
    assert_eq!(read_only.retry(), Retry::Never);
    // A proxies folder that can't be written: the job is refused with the OS's reason, not taken for a bad source.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("clip.mp4");
    std::fs::write(&src, b"a stand-in for a video").unwrap();
    let Some(root) = read_only_dir(dir.path()) else {
        return;
    };
    let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
    let opened = ProxyJob::open_with(
        &root,
        &src.to_string_lossy(),
        Some(&facts(1920, 1080, 2.0)),
        &policy,
    );
    writable_again(&root);
    match opened {
        Opened::Refused(Refused::CacheUnwritable(reason)) => assert!(!reason.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_source_that_isnt_there_now_is_tried_again_when_it_is() {
    let root = tempfile::tempdir().unwrap();
    let proxies = root.path().join("proxies");
    let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
    // On a drive that isn't plugged in: not Stale, tried again when it is there.
    let unplugged = root.path().join("Unplugged").join("clip.mp4");
    let opened = ProxyJob::open_with(
        &proxies,
        &unplugged.to_string_lossy(),
        Some(&facts(1920, 1080, 2.0)),
        &policy,
    );
    assert!(
        matches!(opened, Opened::Refused(Refused::SourceUnavailable)),
        "{opened:?}"
    );
    assert_eq!(Refused::SourceUnavailable.retry(), Retry::WhenPresent);
    // A file that is there but isn't video can never have a copy: Stale.
    let junk = root.path().join("notes.mp4");
    std::fs::write(&junk, b"not a video at all").unwrap();
    let opened = ProxyJob::open_for(&proxies, &junk.to_string_lossy(), &policy);
    assert!(matches!(opened, Opened::Stale), "{opened:?}");
    // A source that goes missing while it is copied keeps its segments and continues when it is back.
    assert_eq!(FailCode::Missing.retry(), Retry::WhenPresent);
    assert_eq!(FailCode::CacheUnwritable.retry(), Retry::Never);
}

#[test]
fn a_source_that_doesnt_state_its_length_is_measured() {
    let f = probe_facts(&fixture("bars-no-length.mkv")).unwrap();
    assert!(
        (f.duration_us - secs(4.0)).abs() <= secs(1.0 / 30.0),
        "{f:?}"
    );
    // Its last keyframe's gap runs to the measured end, not to 0 (within a frame or two: with B-frames, Matroska's
    // decode times sit before the presentation times the length is measured in).
    assert!(
        (f.max_key_interval_us - secs(0.5)).abs() <= secs(0.1),
        "{f:?}"
    );
}

#[test]
fn a_long_source_without_a_length_has_its_keyframes_read_from_the_start() {
    // 40 s, over 16 MiB: the length comes from the file's tail, the keyframe spacing from its first 30 s (not from the
    // tail, whose last keyframe would seem to be followed by a long gap).
    let f = probe_facts(&fixture("bars-looped-no-length.mkv")).unwrap();
    assert!(
        (f.duration_us - secs(40.0)).abs() <= secs(1.0 / 30.0),
        "{f:?}"
    );
    assert!(
        (f.max_key_interval_us - secs(0.5)).abs() <= secs(0.1),
        "{f:?}"
    );
}

#[test]
fn an_unknown_length_is_refused_never_a_one_frame_copy() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("live.webm");
    std::fs::write(&src, b"a stand-in for a video").unwrap();
    let source = src.to_string_lossy().to_string();
    let unknown = SourceFacts {
        duration_us: 0,
        ..facts(1920, 1080, 2.0)
    };
    let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
    let proxies = root.path().join("proxies");
    let refused = ProxyJob::create(&proxies, &source, &unknown, &policy).unwrap_err();
    assert_eq!(refused, Refused::UnknownLength);
    assert_eq!(refused.retry(), Retry::Never);
    let opened = ProxyJob::open_with(&proxies, &source, Some(&unknown), &policy);
    assert!(
        matches!(opened, Opened::Refused(Refused::UnknownLength)),
        "{opened:?}"
    );
    assert!(!proxies.exists(), "nothing is written for it");
}

/// Fails every segment: its file can't be written (`Io` from the encoder thread).
struct Unwritable;

impl Segments for Unwritable {
    fn render(&mut self, _: &ProxyJob, _: u32) -> Result<(u64, u32), RenderError> {
        Err(RenderError::CacheUnwritable(
            "Read-only file system (os error 30)".into(),
        ))
    }
}

/// The encoder is taken (an export, a screen recorder).
struct Busy;

impl Segments for Busy {
    fn render(&mut self, _: &ProxyJob, _: u32) -> Result<(u64, u32), RenderError> {
        Err(RenderError::EncoderBusy)
    }
}

#[test]
fn a_cache_that_cant_be_written_fails_with_its_reason_and_a_busy_encoder_pauses() {
    let root = tempfile::tempdir().unwrap();
    let mut job = fake_job(root.path(), 2);
    let plenty = ProxyEnv::fixed(Thermal::Nominal, |_| 100 << 30);
    let outcome = run_segments(&mut job, &plenty, &Control::default(), &mut Unwritable);
    match &outcome {
        RunOutcome::Failed {
            code: FailCode::CacheUnwritable,
            missing: false,
            message,
        } => assert!(message.contains("Read-only file system"), "{message}"),
        other => panic!("{other:?}"),
    }
    // Only a busy encoder pauses, to be tried again.
    let outcome = run_segments(&mut job, &plenty, &Control::default(), &mut Busy);
    assert_eq!(outcome, RunOutcome::Paused(PauseReason::EncoderBusy));
}

#[cfg(unix)]
#[test]
fn an_output_file_that_cant_be_written_is_an_io_error_not_the_encoders() {
    use klipolo_engine::export::protocol::ErrorCode;
    use klipolo_engine::export::quality::Container;
    // The file layer the segment writer and the join share: creating the output in a read-only folder.
    let dir = tempfile::tempdir().unwrap();
    let Some(read_only) = read_only_dir(dir.path()) else {
        return;
    };
    let joined = read_only.join("joined.mp4");
    let failed = klipolo_engine::export::finalize::join_files(
        &[(fixture("bars-30fps-0.5gop.mp4"), 0)],
        None,
        &joined,
        Container::Mp4,
        FrameRate::whole(30),
        120,
        &mut || {},
    );
    writable_again(&read_only);
    let e = failed.unwrap_err();
    assert_eq!(e.code, ErrorCode::Io, "{}", e.message);
}

#[test]
fn a_resume_finds_missing_segments_and_verifies_only_the_last_again() {
    let root = tempfile::tempdir().unwrap();
    let mut job = fake_job(root.path(), 3);
    let plenty = ProxyEnv::fixed(Thermal::Nominal, |_| 100 << 30);
    let outcome = run_segments(
        &mut job,
        &plenty,
        &Control::default(),
        &mut FakeSegments::default(),
    );
    assert_eq!(outcome, RunOutcome::Finished);
    // Each segment's CRC is in the manifest as it verified.
    let saved = ProxyJob::open(&job.dir).unwrap();
    for s in &saved.segments {
        assert_eq!(s.checksum, checksum(&saved.path(&s.file)).unwrap());
    }
    // Segment 0's file is gone; segment 1's bytes change in place (same size: a resume doesn't read it again); segment 2,
    // the last finished, is verified in full (these stand-ins aren't video, so it fails).
    std::fs::remove_file(saved.path(&saved.segments[0].file)).unwrap();
    std::fs::write(saved.path(&saved.segments[1].file), b"segment X").unwrap();
    let mut resumed = ProxyJob::open(&job.dir).unwrap();
    assert_eq!(resumed.recheck(), vec![0, 2]);
    let again = ProxyJob::open(&job.dir).unwrap();
    assert_eq!(again.done_segments(), 1, "saved");
    assert_eq!(again.next_pending(), Some(0));
}

// ---------------------------------------------------------------------------------------------------------------
// The real pipeline. Needs a GPU and VideoToolbox: `cargo test -p klipolo-engine --test proxy -- --ignored --nocapture`.

mod real {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use klipolo_engine::export::protocol::Lines;
    use klipolo_engine::export::shared::{Control, checksum};
    use klipolo_engine::media::key::{CacheKind, stem};
    use klipolo_engine::media::proxy::decide::ProxyPolicy;
    use klipolo_engine::media::proxy::facts::probe_facts;
    use klipolo_engine::media::proxy::job::{ProxyJob, thumbs_root_for};
    use klipolo_engine::media::proxy::protocol::{ProxyLine, parse_line};
    use klipolo_engine::media::proxy::run::{PauseReason, RunOutcome};
    use klipolo_engine::media::proxy::worker::run_job;
    use klipolo_engine::media::thumbs::{Fill, chunk_path};
    use klipolo_engine::platform::gpu::GpuClass;

    use super::support::fixture;

    /// A job copying `source` (copied into `dir`, so its caches are this test's own) under the Intel policy.
    fn job_for(dir: &Path, source: &Path) -> ProxyJob {
        ffmpeg::init().unwrap();
        let src = dir.join(source.file_name().unwrap());
        std::fs::copy(source, &src).unwrap();
        let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
        let facts = probe_facts(&src).unwrap();
        ProxyJob::create(
            &dir.join("proxies"),
            &src.to_string_lossy(),
            &facts,
            &policy,
        )
        .unwrap()
    }

    fn lines(out: &[u8]) -> Vec<ProxyLine> {
        String::from_utf8_lossy(out)
            .lines()
            .filter_map(parse_line)
            .collect()
    }

    /// Every video packet's (pts, dts, keyframe) in file order, and the stream's time base.
    fn packets(path: &Path) -> (Vec<(i64, i64, bool)>, ffmpeg::Rational) {
        let mut input = ffmpeg::format::input(path).unwrap();
        let video = input.streams().best(ffmpeg::media::Type::Video).unwrap();
        let (index, tb) = (video.index(), video.time_base());
        let packets = input
            .packets()
            .filter(|(s, _)| s.index() == index)
            .map(|(_, p)| (p.pts().unwrap(), p.dts().unwrap(), p.is_key()))
            .collect();
        (packets, tb)
    }

    /// The mean luma of decoded frame `n` (limited range, as stored).
    fn mean_luma(path: &Path, n: usize) -> f64 {
        let mut input = ffmpeg::format::input(path).unwrap();
        let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
        let index = stream.index();
        let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .unwrap()
            .decoder()
            .video()
            .unwrap();
        let (mut frame, mut seen) = (ffmpeg::frame::Video::empty(), 0usize);
        let mut mean = None;
        let mut take = |decoder: &mut ffmpeg::decoder::Video, mean: &mut Option<f64>| {
            while decoder.receive_frame(&mut frame).is_ok() {
                if seen == n {
                    let (w, h, stride) = (
                        frame.width() as usize,
                        frame.height() as usize,
                        frame.stride(0),
                    );
                    let data = frame.data(0);
                    let sum: u64 = (0..h)
                        .flat_map(|y| {
                            data[y * stride..y * stride + w]
                                .iter()
                                .map(|&v| u64::from(v))
                        })
                        .sum();
                    *mean = Some(sum as f64 / (w * h) as f64);
                }
                seen += 1;
            }
        };
        for (s, packet) in input.packets() {
            if s.index() == index {
                decoder.send_packet(&packet).unwrap();
                take(&mut decoder, &mut mean);
                if mean.is_some() {
                    break;
                }
            }
        }
        if mean.is_none() {
            decoder.send_eof().unwrap();
            take(&mut decoder, &mut mean);
        }
        mean.unwrap_or_else(|| panic!("no frame {n} in {}", path.display()))
    }

    fn modified(path: &Path) -> std::time::SystemTime {
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "GPU and VideoToolbox: run by hand"]
    async fn an_editing_copy_has_short_gops_no_sound_and_the_same_timing() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture("longgop-20s-1080p.mp4");
        let job = job_for(dir.path(), &source);
        let mut out = Vec::new();
        let started = Instant::now();
        let outcome = run_job(
            &job.dir,
            &mut Lines::new(&mut out),
            Arc::new(Control::default()),
        )
        .await;
        let elapsed = started.elapsed().as_secs_f64();
        let lines = lines(&out);
        assert_eq!(outcome, RunOutcome::Finished, "{lines:?}");
        assert!(
            matches!(lines.last(), Some(ProxyLine::Done { path }) if *path == job.out),
            "{lines:?}"
        );
        assert!(
            !job.dir.exists(),
            "the job folder is removed once the copy is in place"
        );

        let input = ffmpeg::format::input(&job.out).unwrap();
        assert_eq!(input.streams().count(), 1, "video only");
        let video = input.streams().best(ffmpeg::media::Type::Video).unwrap();
        // SAFETY: the parameters belong to `input`, alive here; plain fields.
        let (w, h, delay) = unsafe {
            let par = video.parameters().as_ptr();
            ((*par).width, (*par).height, (*par).video_delay)
        };
        assert_eq!((w, h), (1280, 720));
        assert_eq!(delay, 0, "no B-frames");
        drop(input);
        let (packets, tb) = packets(&job.out);
        assert_eq!(packets.len(), 600);
        let frame_of = |pts: i64| {
            (pts as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator()) * 30.0).round()
                as usize
        };
        let keys: Vec<usize> = packets
            .iter()
            .filter(|p| p.2)
            .map(|p| frame_of(p.0))
            .collect();
        assert_eq!(
            keys,
            (0..600).step_by(15).collect::<Vec<_>>(),
            "a keyframe every 15 frames"
        );
        assert!(
            packets.windows(2).all(|w| w[0].0 < w[1].0),
            "frames in presentation order: no B-frames"
        );
        let (frames, seconds) = klipolo_engine::export::segment::probe(&job.out).unwrap();
        let (_, source_seconds) = klipolo_engine::export::segment::probe(&source).unwrap();
        assert_eq!(frames, 600);
        assert!(
            (seconds - source_seconds).abs() <= 1.0 / 30.0,
            "{seconds} vs {source_seconds}"
        );
        let (copy, original) = (mean_luma(&job.out, 300), mean_luma(&source, 300));
        assert!(
            (copy - original).abs() <= 3.0,
            "frame 300's luma {copy:.2} vs {original:.2}"
        );
        let thumbs = stem(&job.source, &CacheKind::Thumbs).unwrap();
        let root = thumbs_root_for(job.out.parent().unwrap());
        assert!(
            chunk_path(&root, &thumbs, 0, Fill::Full).exists(),
            "thumbnails chunk 0 refilled"
        );
        assert!(lines.contains(&ProxyLine::Thumbs { chunk: 0 }));

        let bytes = std::fs::metadata(&job.out).unwrap().len();
        eprintln!(
            "copy of 20 s 1080p30 → 720p: {elapsed:.2} s ({:.2}× realtime), {:.1} MB ({:.1} MB per minute)",
            20.0 / elapsed,
            bytes as f64 / 1e6,
            bytes as f64 / 1e6 * 3.0
        );
    }

    /// On the real pipeline: a 23.976 source's copy is made at its exact rate, so at every
    /// timeline frame of a 24, 25, 30 or 60 fps project, at 1× and 1.5×, the preview reading the copy shows the frame
    /// the original shows (each frame's luma names it). The copy carries its keyframe index beside it: opening its
    /// decoders builds none (no whole-file demux).
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "GPU and VideoToolbox: run by hand"]
    async fn a_film_rate_copy_shows_the_frames_the_original_shows() {
        use klipolo_engine::media::keyframes::load_beside;
        use klipolo_engine::plan::{
            MediaCopy, MediaMap, PlanContext, SourceRef, frame_plan, frame_start_us,
        };
        use klipolo_engine::render::pool::{DecodedSet, DecoderPool};
        use klipolo_engine::vendor::cap::video_decode::KeyframeIndex;
        use klipolo_model::project::FormatKind::Landscape;

        let dir = tempfile::tempdir().unwrap();
        let job = job_for(dir.path(), &fixture("film-luma-10s.mp4"));
        assert_eq!(
            (job.rate.num, job.rate.den),
            (24_000, 1001),
            "the copy keeps the source's exact rate"
        );
        let mut out = Vec::new();
        let outcome = run_job(
            &job.dir,
            &mut Lines::new(&mut out),
            Arc::new(Control::default()),
        )
        .await;
        assert_eq!(outcome, RunOutcome::Finished, "{:?}", lines(&out));
        assert!(
            load_beside(&job.out).is_some(),
            "the copy's index is beside it"
        );
        let fps = job.rate.f64();
        let (src, copy): (Arc<str>, Arc<str>) = (
            job.source.as_str().into(),
            job.out.to_string_lossy().as_ref().into(),
        );
        // What the preview shows at a plan's source time, read through `path` (the pool numbers its frames at `fps`).
        let luma = async |pool: &mut DecoderPool, path: &Arc<str>, time_us: i64| {
            let source = SourceRef {
                asset_id: "broll".into(),
                path: path.clone(),
                original: src.clone(),
                time_us,
                fps,
                rotation: Some(0),
                width: 1280,
                height: 720,
            };
            let plan = klipolo_engine::plan::FramePlan {
                format: Landscape,
                canvas: (1280, 720),
                at_us: 0,
                background: klipolo_engine::plan::BackgroundPlan::Solid { color: [0; 3] },
                layers: vec![klipolo_engine::plan::Layer::Video(
                    klipolo_engine::plan::VideoLayer {
                        role: klipolo_engine::plan::VideoRole::Main,
                        id: "b".into(),
                        source,
                        dest: klipolo_engine::plan::framing::FULL,
                        crop: klipolo_engine::plan::framing::FULL,
                        rotation: 0.0,
                        opacity: 1.0,
                        radius: 0.0,
                        mirror: false,
                        border: false,
                        look: None,
                        effects: Vec::new(),
                        pad_px: 0.0,
                        blend: klipolo_model::motion::Blend::Normal,
                        reach: 1.0,
                    },
                )],
                missing: Vec::new(),
            };
            let mut set = DecodedSet::default();
            pool.decode(&plan, &mut set).await;
            let (frame, _) = set.get(0).expect("a frame");
            let y = frame.y_plane().expect("NV12");
            let (cx, cy) = (frame.width() as usize / 2, frame.height() as usize / 2);
            i32::from(y[cy * frame.y_stride() as usize + cx])
        };
        // The source times the plan asks for, through the copy and as export reads the original.
        let mut times = Vec::new();
        for project_fps in [24, 25, 30, 60] {
            for speed in [1.0, 1.5] {
                let mut p = super::support::project();
                p.settings.fps = project_fps;
                let b = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
                (b.path, b.fps, b.width, b.height) = (src.to_string(), fps, 1280, 720);
                p.timeline
                    .main
                    .iter_mut()
                    .find(|c| c.id == "b")
                    .unwrap()
                    .speed = speed;
                let map = Arc::new(MediaMap::from([(
                    src.clone(),
                    MediaCopy::new(copy.clone(), fps),
                )]));
                let original = PlanContext::new(Arc::new(p.clone()));
                let preview = PlanContext::new(Arc::new(p)).with_media(map);
                let start = original.starts[1];
                for f in 0..(2 * i64::from(project_fps)) {
                    let at = start + frame_start_us(f, project_fps);
                    let video = |ctx: &PlanContext| {
                        frame_plan(ctx, at, Landscape)
                            .layers
                            .iter()
                            .find_map(|l| match l {
                                klipolo_engine::plan::Layer::Video(v) => Some(v.source.clone()),
                                _ => None,
                            })
                            .unwrap()
                    };
                    let (o, c) = (video(&original), video(&preview));
                    assert_eq!((c.path.as_ref(), c.time_us), (copy.as_ref(), o.time_us));
                    times.push((project_fps, speed, f, o.time_us));
                }
            }
        }
        // The copy first, alone: no index is built for it.
        let builds = KeyframeIndex::builds();
        let mut pool = DecoderPool::new(klipolo_engine::render::PREVIEW_TUNING, false);
        let mut shown = Vec::with_capacity(times.len());
        for &(.., t) in &times {
            shown.push(luma(&mut pool, &copy, t).await);
        }
        assert_eq!(
            KeyframeIndex::builds(),
            builds,
            "opening the copy demuxed it whole"
        );
        drop(pool);
        let mut pool = DecoderPool::new(klipolo_engine::render::PREVIEW_TUNING, false);
        for (&(project_fps, speed, f, t), got) in times.iter().zip(shown) {
            let want = luma(&mut pool, &src, t).await;
            assert!(
                (got - want).abs() <= 3,
                "{project_fps} fps at {speed}×, timeline frame {f} ({t} µs): the copy shows luma {got}, the original {want}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "GPU and VideoToolbox: run by hand"]
    async fn stopping_and_resuming_keeps_finished_segments() {
        let dir = tempfile::tempdir().unwrap();
        let job = job_for(dir.path(), &fixture("longgop-20s-1080p.mp4"));
        // Stop once the first segment is verified (mid-way through the second).
        let control = Arc::new(Control::default());
        let (watched, pauser) = (job.dir.clone(), control.clone());
        let watch = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if ProxyJob::open(&watched).is_ok_and(|j| j.done_segments() >= 1) {
                    pauser.pause();
                    return;
                }
            }
        });
        let first = run_job(&job.dir, &mut Lines::new(Vec::new()), control).await;
        watch.abort();
        assert_eq!(first, RunOutcome::Paused(PauseReason::User));
        let paused = ProxyJob::open(&job.dir).unwrap();
        assert_eq!(paused.done_segments(), 1);
        let seg0 = paused.segments[0].clone();
        let path0 = paused.path(&seg0.file);
        let unfinished: PathBuf = paused.path(&paused.segments[1].file);
        assert!(
            !unfinished.exists(),
            "the stopped segment's partial file is deleted"
        );
        // Keep segment 0's file past the job folder's removal (a hard link), and make it read-only: a resumed run
        // that tried to write it again would fail.
        let kept = dir.path().join("seg0.mp4");
        std::fs::hard_link(&path0, &kept).unwrap();
        let mtime = modified(&kept);
        let mut perms = std::fs::metadata(&path0).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path0, perms).unwrap();

        let mut out = Vec::new();
        let second = run_job(
            &job.dir,
            &mut Lines::new(&mut out),
            Arc::new(Control::default()),
        )
        .await;
        let lines = lines(&out);
        assert_eq!(second, RunOutcome::Finished, "{lines:?}");
        assert_eq!(
            lines.first(),
            Some(&ProxyLine::Progress {
                done: 300,
                total: 600
            }),
            "it starts from segment 1"
        );
        assert_eq!(
            checksum(&kept).unwrap(),
            seg0.checksum,
            "segment 0 unchanged"
        );
        assert_eq!(modified(&kept), mtime);
        let (frames, _) = klipolo_engine::export::segment::probe(&job.out).unwrap();
        assert_eq!(frames, 600, "the delivered copy plays through");
        // The resumed run started inside chunk 0 (at 10 s), so it couldn't fill it; the chunk
        // is drawn from the finished copy before `Done`, and the filmstrip told.
        let thumbs = lines
            .iter()
            .position(|l| *l == ProxyLine::Thumbs { chunk: 0 });
        let done = lines
            .iter()
            .position(|l| matches!(l, ProxyLine::Done { .. }));
        assert!(thumbs.is_some() && thumbs < done, "{lines:?}");
        let stem = job.thumbs_stem.as_ref().unwrap();
        assert!(chunk_path(&job.thumbs_root, stem, 0, Fill::Full).exists());
    }

    /// A segment damaged in place (same size, so the resume's cheap check passes it) makes the join fail its check: every
    /// segment is then verified in full, the damaged one is made again and the copy is delivered, rather than the job
    /// failing on every retry.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "GPU and VideoToolbox: run by hand"]
    async fn a_damaged_middle_segment_is_made_again_when_the_join_finds_it() {
        use klipolo_engine::media::proxy::facts::SourceFacts;
        ffmpeg::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("scan.mp4");
        std::fs::copy(fixture("scan-130s-720p.mp4"), &src).unwrap();
        // The first 40 s: four segments.
        let facts = SourceFacts {
            duration_us: 40_000_000,
            ..probe_facts(&src).unwrap()
        };
        let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
        let job = ProxyJob::create(
            &dir.path().join("proxies"),
            &src.to_string_lossy(),
            &facts,
            &policy,
        )
        .unwrap();
        assert_eq!(job.segments.len(), 4);
        // Stop once three segments are finished.
        let control = Arc::new(Control::default());
        let (watched, pauser) = (job.dir.clone(), control.clone());
        let watch = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if ProxyJob::open(&watched).is_ok_and(|j| j.done_segments() >= 3) {
                    pauser.pause();
                    return;
                }
            }
        });
        let first = run_job(&job.dir, &mut Lines::new(Vec::new()), control).await;
        watch.abort();
        assert_eq!(first, RunOutcome::Paused(PauseReason::User));
        let paused = ProxyJob::open(&job.dir).unwrap();
        assert_eq!(paused.done_segments(), 3);
        // Segment 1's first bytes are lost in place (a media error): same size, no longer readable.
        let middle = paused.path(&paused.segments[1].file);
        let mut bytes = std::fs::read(&middle).unwrap();
        bytes[..64].fill(0);
        std::fs::write(&middle, &bytes).unwrap();
        let mut out = Vec::new();
        let second = run_job(
            &job.dir,
            &mut Lines::new(&mut out),
            Arc::new(Control::default()),
        )
        .await;
        assert_eq!(second, RunOutcome::Finished, "{:?}", lines(&out));
        let (frames, _) = klipolo_engine::export::segment::probe(&job.out).unwrap();
        assert_eq!(frames, 1200, "the copy plays through");
    }

    /// Measures a copy of `$KLIPOLO_PROXY_BENCH` (any video; skipped when unset): speed, size and the worker's own log.
    /// Run under `/usr/bin/time -l` for CPU and peak memory.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "GPU and VideoToolbox: a benchmark, run by hand"]
    async fn bench_an_editing_copy() {
        let Some(source) = std::env::var_os("KLIPOLO_PROXY_BENCH").map(PathBuf::from) else {
            eprintln!("skipped: set KLIPOLO_PROXY_BENCH to a video");
            return;
        };
        ffmpeg::init().unwrap();
        let facts = probe_facts(&source).unwrap();
        let reason = |class| {
            klipolo_engine::media::proxy::decide::needs_proxy(
                &facts,
                &ProxyPolicy::with_encoder(class, true),
            )
        };
        eprintln!(
            "bench facts: {facts:?}; needs a copy: Intel {:?}, Apple {:?}",
            reason(GpuClass::Integrated),
            reason(GpuClass::Apple)
        );
        let dir = tempfile::tempdir().unwrap();
        let job = job_for(dir.path(), &source);
        let started = Instant::now();
        let outcome = run_job(
            &job.dir,
            &mut Lines::new(Vec::new()),
            Arc::new(Control::default()),
        )
        .await;
        let elapsed = started.elapsed().as_secs_f64();
        assert_eq!(outcome, RunOutcome::Finished);
        let seconds = job.duration_us as f64 / 1e6;
        let bytes = std::fs::metadata(&job.out).unwrap().len() as f64;
        eprintln!(
            "bench: {seconds:.1} s of {}×{} → {}×{}: {elapsed:.2} s ({:.2}× realtime, {:.1} s per minute), {:.1} MB per minute, {:.2} GB per hour",
            job.source_width,
            job.source_height,
            job.width,
            job.height,
            seconds / elapsed,
            elapsed * 60.0 / seconds,
            bytes / 1e6 * 60.0 / seconds,
            bytes / 1e9 * 3600.0 / seconds
        );
        // Scrubbing before and after: random seeks through the preview's decoder (its main box), on the source and on
        // its copy (`KLIPOLO_PROXY_BENCH_NO_SEEKS` skips them, so `time -l` measures the copy alone).
        if std::env::var_os("KLIPOLO_PROXY_BENCH_NO_SEEKS").is_some() {
            return;
        }
        for (name, path, fps) in [
            ("source", PathBuf::from(&job.source), job.source_fps),
            ("copy", job.out.clone(), job.rate.f64()),
        ] {
            let (p50, p95) = seeks(&path, fps, job.duration_us).await;
            eprintln!("bench seeks on the {name}: p50 {p50:.1} ms, p95 {p95:.1} ms");
        }
    }

    /// The worker as the app runs it (`--proxy-worker`: its own process at utility QoS), through `ProxyRunner`: it is
    /// paused once a segment is finished, resumed by starting again on the folder, and delivers the copy; a second job
    /// is stopped, which deletes its folder. The worker binary is `$KLIPOLO_WORKER_EXE`, else this checkout's
    /// `target/debug/examples/perf_harness` (`cargo build -p klipolo-engine --example perf_harness`). The source is
    /// `$KLIPOLO_PROXY_BENCH`, else `longgop-20s-1080p.mp4`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "GPU, VideoToolbox and the perf_harness example: run by hand"]
    async fn the_runner_pauses_resumes_and_stops_a_worker_process() {
        use klipolo_engine::media::proxy::runner::{ProcessLauncher, ProxyEvent, ProxyRunner};
        let exe = std::env::var_os("KLIPOLO_WORKER_EXE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/examples/perf_harness")
            });
        assert!(
            exe.exists(),
            "build it: cargo build -p klipolo-engine --example perf_harness"
        );
        let launcher = ProcessLauncher::proxy(exe);
        let source = std::env::var_os("KLIPOLO_PROXY_BENCH")
            .map(PathBuf::from)
            .unwrap_or_else(|| fixture("longgop-20s-1080p.mp4"));
        let dir = tempfile::tempdir().unwrap();
        let job = job_for(dir.path(), &source);

        // Pause once the first segment is in the manifest.
        let mut runner = ProxyRunner::start(&job.dir, &launcher).unwrap();
        let mut paused_events = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(120), runner.next())
                .await
                .expect("an event within two minutes")
                .expect("the worker is still running");
            paused_events.push(event);
            if ProxyJob::open(&job.dir).is_ok_and(|j| j.done_segments() >= 1) {
                break;
            }
        }
        runner.pause().await;
        while let Some(event) = runner.next().await {
            paused_events.push(event);
        }
        assert_eq!(
            paused_events.last(),
            Some(&ProxyEvent::Paused {
                reason: PauseReason::User
            }),
            "{paused_events:?}"
        );
        let done_before = ProxyJob::open(&job.dir).unwrap().done_segments();
        assert!(done_before >= 1);

        // Resume: a new worker on the same folder, measured end to end (process start and GPU set-up included).
        let started = Instant::now();
        let mut runner = ProxyRunner::start(&job.dir, &launcher).unwrap();
        let mut events = Vec::new();
        while let Some(event) = runner.next().await {
            events.push(event);
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert_eq!(
            events.last(),
            Some(&ProxyEvent::Done {
                path: job.out.clone()
            }),
            "{events:?}"
        );
        assert!(
            matches!(events.first(), Some(ProxyEvent::Progress { done, .. }) if *done >= 300),
            "it resumes from its finished segments: {events:?}"
        );
        let remaining =
            job.duration_us as f64 / 1e6 * (1.0 - done_before as f64 / job.segments.len() as f64);
        eprintln!(
            "worker process: {remaining:.1} s of video in {elapsed:.2} s ({:.2}× realtime, process start included)",
            remaining / elapsed
        );

        // Stop: the worker is killed and its folder deleted.
        let other = tempfile::tempdir().unwrap();
        let job = job_for(other.path(), &source);
        let mut runner = ProxyRunner::start(&job.dir, &launcher).unwrap();
        let first = tokio::time::timeout(Duration::from_secs(120), runner.next()).await;
        assert!(
            matches!(first, Ok(Some(ProxyEvent::Progress { .. }))),
            "{first:?}"
        );
        runner.stop().await;
        assert!(!job.dir.exists(), "a stopped copy's folder is deleted");
        assert!(!job.out.exists());
    }

    /// Seek latency (p50, p95 ms) of seeded random seeks: 3 blocks of 30, each on a fresh decoder tuned like the
    /// preview's main footage, after an untimed first frame (as `decode.rs`'s `scrub_p95`).
    async fn seeks(path: &Path, fps: f64, duration_us: i64) -> (f64, f64) {
        use klipolo_engine::vendor::cap::rendering::decoder::{DecoderTuning, spawn_decoder};
        let frames = (duration_us as f64 * fps / 1e6) as u64;
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut ms = Vec::new();
        for _ in 0..3 {
            let tuning = DecoderTuning {
                idle_timeout: Duration::from_secs(5),
                max_cache_bytes: 96 << 20,
                max_output: Some((1920, 1080)),
                ..DecoderTuning::default()
            };
            let decoder = spawn_decoder("bench", path.to_path_buf(), fps, 0.0, false, tuning)
                .await
                .unwrap();
            decoder.get_frame_initial(0.0).await;
            for _ in 0..30 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let t = (seed % frames.max(1)) as f64 / fps;
                let started = Instant::now();
                if decoder.get_frame(t as f32).await.is_none() {
                    break;
                }
                ms.push(started.elapsed().as_secs_f64() * 1e3);
            }
        }
        ms.sort_by(f64::total_cmp);
        let at = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
        (at(0.5), at(0.95))
    }
}
