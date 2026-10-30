//! Joining a format's segments and the sound track into its file. It is a stream copy, with no re-encode, into
//! a file with no edit list (YouTube's guidance): the first frame shows at 0, through negative composition offsets
//! (`ctts` version 1) for B-frames, and the sound starts at 0 (its priming is compensated in `sound.rs`).
//!
//! MP4s get their index at the front in space reserved while joining (`moov_size`), so no second full rewrite is
//! needed (`+faststart` only if the estimate was short). Master's `.mov` keeps its index at the end. Tracks are
//! interleaved in about half-second chunks. The result is verified before it counts (frame count, duration, both
//! tracks starting at 0, no edit list, index first), then the format's segments are deleted.
//!
//! Editing copies join their video-only segments the same way (`join_files` without a sound file).

use std::path::{Path, PathBuf};
use std::time::Instant;

use ffmpeg::packet::Mut;
use ffmpeg::{Packet, Rational, format, media};

use super::boxes::{read_range, top_level_boxes};
use super::quality::Container;
use super::shared::{FrameRate, JobHandle, WorkerError, file_error, internal, io_error};

fn broken(message: impl std::fmt::Display) -> WorkerError {
    internal(format!("joining: {message}"))
}

/// Timestamps in `from` → `to`.
fn rescale(v: i64, from: Rational, to: Rational) -> i64 {
    let num = i128::from(v) * i128::from(from.numerator()) * i128::from(to.denominator());
    let den = i128::from(from.denominator()) * i128::from(to.numerator());
    (num / den.max(1)) as i64
}

/// Each track is written in runs of about this long (fewer, larger chunks: a smaller index, fewer seeks).
const CHUNK_S: f64 = 0.5;

fn seconds(v: i64, tb: Rational) -> f64 {
    v as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator().max(1))
}

/// Reads `input`'s next packet into `packet`, first letting go of the one it held. FFmpeg 9 still resets the packet
/// with `av_init_packet` inside `av_read_frame` (`FF_API_INIT_PACKET`, libavcodec < 64) without unreferencing it, so a
/// packet reused across reads leaked every buffer it had held: the whole file, for a join.
fn read_next(packet: &mut Packet, input: &mut format::context::Input) -> Result<(), ffmpeg::Error> {
    // Dropping the old packet unreferences its buffer; the new one is a blank struct (no allocation).
    *packet = Packet::empty();
    packet.read(input)
}

/// The segments' video packets in order, re-timed onto the timeline (input time base).
struct Video {
    files: Vec<(PathBuf, u64)>,
    next: usize,
    input: Option<format::context::Input>,
    index: usize,
    offset: i64,
    tb: Rational,
    rate: FrameRate,
}

impl Video {
    /// The next packet into `packet`; false at the end.
    fn read(&mut self, packet: &mut Packet) -> Result<bool, WorkerError> {
        loop {
            if self.input.is_none() {
                let Some((path, start)) = self.files.get(self.next).cloned() else {
                    return Ok(false);
                };
                self.next += 1;
                let input = crate::media::open::input(&path).map_err(|e| broken(format!("{e}")))?;
                let stream = input
                    .streams()
                    .best(media::Type::Video)
                    .ok_or_else(|| broken("a segment without video"))?;
                (self.index, self.tb) = (stream.index(), stream.time_base());
                self.offset = rescale(start as i64, self.rate.frame_base(), self.tb);
                self.input = Some(input);
            }
            let input = self.input.as_mut().expect("open");
            match read_next(packet, input) {
                Ok(()) if packet.stream() == self.index => {
                    packet.set_pts(packet.pts().map(|t| t + self.offset));
                    packet.set_dts(packet.dts().map(|t| t + self.offset));
                    return Ok(true);
                }
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => self.input = None,
                Err(e) => return Err(broken(e)),
            }
        }
    }
}

/// Joins output `output` into its file in the job folder; returns its path and size.
pub fn join(
    job: &JobHandle,
    output: usize,
    heartbeat: &mut dyn FnMut(),
) -> Result<(PathBuf, u64), WorkerError> {
    // Segments from different encoders can't share one track (each has its own parameter sets).
    let mixed = job.read(|j| {
        let mut configs = j.outputs[output].segments.iter().map(|s| s.config);
        let first = configs.next().flatten();
        configs.any(|c| c != first)
    });
    if mixed {
        return Err(broken("its parts come from different encoders"));
    }
    let (files, audio_path, path, container, fps, total) = job.read(|j| {
        let o = &j.outputs[output];
        let files: Vec<(PathBuf, u64)> = o
            .segments
            .iter()
            .map(|s| (j.path(&s.file), s.start_frame))
            .collect();
        (
            files,
            j.path(&j.audio.file),
            j.path(&o.file),
            o.settings.container,
            FrameRate::whole(j.fps),
            j.total_frames,
        )
    });
    let bytes = join_files(
        &files,
        Some(&audio_path),
        &path,
        container,
        fps,
        total,
        heartbeat,
    )?;
    job.update(|j| j.outputs[output].joined = true)?;
    for (file, _) in &files {
        let _ = std::fs::remove_file(file);
    }
    if let Some(dir) = files.first().and_then(|(f, _)| f.parent()) {
        let _ = std::fs::remove_dir(dir);
    }
    Ok((path, bytes))
}

/// Joins `files` (each segment file and the timeline frame it starts at) and, when given, the sound file `audio` into
/// `path`: a stream copy with the index first (MP4) and no edit list, verified (frame count, duration, every track
/// starting at 0). Returns its size. A failed join leaves no file at `path`; the segments are untouched.
pub fn join_files(
    files: &[(PathBuf, u64)],
    audio: Option<&Path>,
    path: &Path,
    container: Container,
    rate: FrameRate,
    total: u64,
    heartbeat: &mut dyn FnMut(),
) -> Result<u64, WorkerError> {
    let mut result = write(files, audio, path, container, rate, total, false, heartbeat);
    if matches!(result, Ok(false)) {
        tracing::info!("join: the reserved index space was short, rewriting with faststart");
        result = write(files, audio, path, container, rate, total, true, heartbeat);
    }
    match result.and_then(|_| verify(path, total, rate, container, audio.is_some())) {
        Ok(bytes) => Ok(bytes),
        Err(e) => {
            let _ = std::fs::remove_file(path);
            Err(e)
        }
    }
}

/// Writes the joined file. MP4: the index goes into reserved space (`rewrite`: by FFmpeg's second pass instead).
/// False when the reserved space was too small.
#[allow(clippy::too_many_arguments)]
fn write(
    files: &[(PathBuf, u64)],
    audio_path: Option<&Path>,
    path: &Path,
    container: Container,
    rate: FrameRate,
    total: u64,
    rewrite: bool,
    heartbeat: &mut dyn FnMut(),
) -> Result<bool, WorkerError> {
    let first = crate::media::open::input(&files.first().ok_or_else(|| broken("no segments"))?.0)
        .map_err(broken)?;
    let mut audio = match audio_path {
        Some(p) => Some(crate::media::open::input(p).map_err(broken)?),
        None => None,
    };
    let vin = first
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| broken("no video"))?;
    // The sound's stream index, time base, parameters and packet count.
    let sound = match &audio {
        Some(input) => {
            let ain = input
                .streams()
                .best(media::Type::Audio)
                .ok_or_else(|| broken("no sound"))?;
            Some((
                ain.index(),
                ain.time_base(),
                ain.parameters(),
                u64::try_from(ain.frames()).unwrap_or(0),
            ))
        }
        None => None,
    };
    let (a_index, a_tb) = sound
        .as_ref()
        .map_or((0, Rational(1, 48_000)), |(i, tb, ..)| (*i, *tb));
    // The index: per sample a size, a composition offset and a chunk share; per keyframe and chunk a little more.
    let audio_packets = sound
        .as_ref()
        .map_or(0, |(.., frames)| (*frames).max(total * 2));
    let reserve = 64 * 1024 + 24 * total + 12 * audio_packets;
    let mut out = format::output_as(
        path,
        match container {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
        },
    )
    .map_err(|e| file_error(e, "creating the video"))?;
    let tracks = std::iter::once((vin.parameters(), vin.time_base())).chain(
        sound
            .iter()
            .map(|(_, tb, parameters, _)| (parameters.clone(), *tb)),
    );
    for (parameters, tb) in tracks {
        let mut stream = out
            .add_stream(None::<ffmpeg::Codec>)
            .map_err(|e| file_error(e, "creating the video"))?;
        stream.set_parameters(parameters);
        stream.set_time_base(tb);
    }
    drop(first);
    let mut options = ffmpeg::Dictionary::new();
    options.set("use_editlist", "0");
    match container {
        Container::Mp4 if rewrite => options.set("movflags", "+faststart+negative_cts_offsets"),
        Container::Mp4 => {
            options.set("movflags", "+negative_cts_offsets");
            options.set("moov_size", &reserve.to_string());
        }
        Container::Mov => options.set("movflags", "+negative_cts_offsets"),
    }
    out.write_header_with(options)
        .map_err(|e| file_error(e, "starting the video"))?;
    let tbs = [
        out.stream(0).map_or(Rational(1, 90_000), |s| s.time_base()),
        out.stream(1).map_or(a_tb, |s| s.time_base()),
    ];
    let mut video = Video {
        files: files.to_vec(),
        next: 0,
        input: None,
        index: 0,
        offset: 0,
        tb: Rational(1, 90_000),
        rate,
    };
    let (mut vp, mut ap) = (Packet::empty(), Packet::empty());
    let (mut have_v, mut have_a) = (
        video.read(&mut vp)?,
        match audio.as_mut() {
            Some(input) => read_audio(input, a_index, &mut ap)?,
            None => false,
        },
    );
    let (mut a_time, mut frames, mut last_dts) = (0i64, 0u64, [i64::MIN; 2]);
    let (mut beat, started) = (Instant::now(), Instant::now());
    let mut on_video = true;
    while have_v || have_a {
        // Stay on a track until it is half a second ahead of the other.
        let (tv, ta) = (
            seconds(vp.dts().unwrap_or(0), video.tb),
            seconds(a_time, a_tb),
        );
        on_video = if on_video {
            tv <= ta + CHUNK_S
        } else {
            tv + CHUNK_S < ta
        };
        let take_video = have_v && (!have_a || on_video);
        let (packet, stream, from) = if take_video {
            frames += 1;
            (&mut vp, 0usize, video.tb)
        } else {
            // The sound is re-timed from 0, packet after packet (the intermediate's timestamps don't matter).
            let duration = if ap.duration() > 0 {
                ap.duration()
            } else {
                1024
            };
            ap.set_pts(Some(a_time));
            ap.set_dts(Some(a_time));
            ap.set_duration(duration);
            a_time += duration;
            (&mut ap, 1usize, a_tb)
        };
        let dts = packet.dts().unwrap_or(0);
        if dts <= last_dts[stream] {
            return Err(broken(format!("stream {stream} goes back at {dts}")));
        }
        last_dts[stream] = dts;
        // SAFETY: a packet this function owns; only its flags change (pre-roll a demuxer marked as "discard" is kept).
        unsafe { (*packet.as_mut_ptr()).flags &= !ffmpeg::ffi::AV_PKT_FLAG_DISCARD };
        packet.set_stream(stream);
        packet.set_position(-1);
        packet.rescale_ts(from, tbs[stream]);
        packet
            .write(&mut out)
            .map_err(|e| file_error(e, "writing the video"))?;
        if take_video {
            have_v = video.read(&mut vp)?;
        } else if let Some(input) = audio.as_mut() {
            have_a = read_audio(input, a_index, &mut ap)?;
        }
        if beat.elapsed().as_secs_f64() >= 1.0 {
            beat = Instant::now();
            heartbeat();
        }
    }
    if frames != total {
        return Err(broken(format!("{frames} of {total} frames")));
    }
    // With `+faststart` this rewrites the file with its index first (a full copy, as one call).
    heartbeat();
    let copied = started.elapsed().as_secs_f64();
    match out.write_trailer() {
        Ok(()) => {}
        Err(ffmpeg::Error::Other { errno }) if errno == libc::EINVAL && !rewrite => {
            return Ok(false);
        }
        Err(e) => return Err(file_error(e, "finishing the video")),
    }
    drop(out);
    let trailer = started.elapsed().as_secs_f64();
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_error(e, "saving the video"))?;
    // The job folder's `<format>.<ext>`: the format, never the folder.
    eprintln!(
        "export join {}: copy {copied:.2} s, trailer {:.2} s, sync {:.2} s",
        crate::log_name(path),
        trailer - copied,
        started.elapsed().as_secs_f64() - trailer
    );
    Ok(true)
}

fn read_audio(
    input: &mut format::context::Input,
    index: usize,
    packet: &mut Packet,
) -> Result<bool, WorkerError> {
    loop {
        match read_next(packet, input) {
            Ok(()) if packet.stream() == index => return Ok(true),
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => return Ok(false),
            Err(e) => return Err(broken(e)),
        }
    }
}

/// Checks the joined file: `total` frames lasting `total / rate`, video (and sound, when `with_sound`) starting at 0,
/// no edit list, and (MP4) the index before the media. Returns its size.
fn verify(
    path: &Path,
    total: u64,
    rate: FrameRate,
    container: Container,
    with_sound: bool,
) -> Result<u64, WorkerError> {
    let (frames, duration) = super::segment::probe(path).ok_or_else(|| broken("unreadable"))?;
    let expected = rate.seconds(total);
    if frames != total || (duration - expected).abs() > 1.5 * rate.seconds(1) {
        return Err(broken(format!(
            "{frames} frames, {duration:.3} s (expected {total}, {expected:.3} s)"
        )));
    }
    let mut input = crate::media::open::input(path).map_err(broken)?;
    // Video and sound; without sound, its start counts as 0.
    let mut starts = [None::<i64>, (!with_sound).then_some(0)];
    let kinds: Vec<media::Type> = input.streams().map(|s| s.parameters().medium()).collect();
    if !with_sound && kinds.contains(&media::Type::Audio) {
        return Err(broken("a sound track in a video-only file"));
    }
    let mut packet = Packet::empty();
    for _ in 0..1_000 {
        if starts.iter().all(Option::is_some) || read_next(&mut packet, &mut input).is_err() {
            break;
        }
        let slot = match kinds.get(packet.stream()) {
            Some(media::Type::Video) => 0,
            Some(media::Type::Audio) => 1,
            _ => continue,
        };
        starts[slot].get_or_insert(packet.pts().unwrap_or(i64::MIN));
    }
    if starts != [Some(0), Some(0)] {
        return Err(broken(format!("tracks start at {starts:?}, not 0")));
    }
    let boxes = top_level_boxes(path).map_err(|e| io_error(e, "reading the video"))?;
    let at = |kind: &[u8; 4]| boxes.iter().position(|(k, _, _)| k == kind);
    let (Some(moov), Some(mdat)) = (at(b"moov"), at(b"mdat")) else {
        return Err(broken("no index or no media"));
    };
    if container == Container::Mp4 && moov > mdat {
        return Err(broken("the index isn't at the front"));
    }
    let (_, offset, size) = boxes[moov];
    let index = read_range(path, offset, size).map_err(|e| io_error(e, "reading the video"))?;
    if index.windows(4).any(|w| w == b"edts" || w == b"elst") {
        return Err(broken("an edit list was written"));
    }
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| io_error(e, "reading the video"))
}
