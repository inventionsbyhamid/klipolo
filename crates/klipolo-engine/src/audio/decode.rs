//! Streaming audio decode for the PCM cache: packet by packet to 48 kHz f32, mono kept mono (anything with more
//! channels becomes stereo), handed out in blocks so nothing holds the whole file. Same FFmpeg setup as the
//! vendored Cap `AudioData::decode` (resampler options, the 2 s pre-roll before a seek point, positions from frame
//! timestamps), which collects every sample in a `Vec` instead.
//!
//! Every stream but the sound is discarded at the demuxer: no other stream becomes packets, and their bytes are skipped
//! where the file's layout allows (runs longer than FFmpeg's 32 KB short-seek window). The pass reads sound only:
//! thumbnails come from a scan of their own after it (`media::store`). A read error (an unplugged drive)
//! fails the decode: it is never taken for the end of the file.

use std::path::Path;

use ffmpeg::software::resampling;
use ffmpeg::{ChannelLayout, codec as avcodec};

use super::mix::RATE;
use crate::media::scan::{next_packet, set_discards};
use crate::vendor::cap::audio_data::{AudioData, ResamplerOutput, high_quality_resampler_options};

/// Decoded audio before a seek point that is decoded and thrown away (a decoder needs it to settle).
const PRE_ROLL: u64 = 2 * RATE as u64;

/// What a file holds, before decoding it.
#[derive(Clone, Copy, Debug)]
pub struct Probe {
    /// 1 or 2 (what the cache stores).
    pub channels: u16,
    /// 48 kHz frames from the container's duration, if it states one.
    pub frames: Option<u64>,
}

pub fn probe(path: &Path) -> Result<Probe, String> {
    let _ = ffmpeg::init();
    let input = crate::media::open::input(&path).map_err(|e| format!("Input Open / {e}"))?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or("No Stream")?;
    let decoder = avcodec::Context::from_parameters(stream.parameters())
        .and_then(|c| c.decoder().audio())
        .map_err(|e| format!("Audio Parameters / {e}"))?;
    let channels = if decoder.channels() <= 1 { 1 } else { 2 };
    let seconds = if stream.duration() > 0 {
        stream.duration() as f64 * f64::from(stream.time_base())
    } else if input.duration() > 0 {
        input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
    } else {
        0.0
    };
    let frames = (seconds > 0.0).then(|| (seconds * f64::from(RATE)).ceil() as u64);
    Ok(Probe { channels, frames })
}

/// Receives each decoded block as (first frame, interleaved samples); false stops the decode.
pub type Emit<'a> = dyn FnMut(u64, &[f32]) -> Result<bool, String> + 'a;

/// Why a decode gave up.
#[derive(Debug)]
pub enum DecodeError {
    /// The container can't seek there, or landed past the point: the caller decodes from the start instead.
    CantSeek,
    /// Reading the file failed part-way (an unplugged drive, a bad sector, a long run of damaged packets): what came
    /// before it was handed out, nothing after it.
    Unreadable(String),
    Failed(String),
}

impl From<String> for DecodeError {
    fn from(e: String) -> Self {
        Self::Failed(e)
    }
}

/// Decodes `[from, until)` (48 kHz frames; `until: None` = to the end) and hands each block to `emit` as (first
/// frame, interleaved samples). `emit` returns false to stop early. `from > 0` seeks there (with pre-roll) and
/// fails with `CantSeek` when that doesn't work.
pub fn decode(
    path: &Path,
    from: u64,
    until: Option<u64>,
    emit: &mut Emit,
) -> Result<(), DecodeError> {
    let input = crate::media::open::input(&path).map_err(|e| format!("Input Open / {e}"))?;
    decode_input(input, from, until, emit)
}

/// [`decode`] of an input already open (a test's, read through a connection that fails part-way: `media::open` reads
/// files only).
pub(crate) fn decode_input(
    mut input: ffmpeg::format::context::Input,
    from: u64,
    until: Option<u64>,
    emit: &mut Emit,
) -> Result<(), DecodeError> {
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or_else(|| "No Stream".to_string())?;
    let mut decoder = avcodec::Context::from_parameters(stream.parameters())
        .and_then(|c| c.decoder().audio())
        .map_err(|e| format!("Audio Parameters / {e}"))?;
    let source_channels = decoder.channels().max(1);
    if decoder.channel_layout().is_empty() {
        decoder.set_channel_layout(ChannelLayout::default(i32::from(source_channels)));
    }
    let time_base = stream.time_base();
    decoder.set_packet_time_base(time_base);
    let channels: u16 = if source_channels <= 1 { 1 } else { 2 };
    let mut resampler = resampling::Context::get_with(
        decoder.format(),
        decoder.channel_layout(),
        decoder.rate(),
        AudioData::SAMPLE_FORMAT,
        ChannelLayout::default(i32::from(channels)),
        RATE,
        high_quality_resampler_options(),
    )
    .map_err(|e| format!("Resampler / {e}"))?;
    let index = stream.index();
    // The cache counts from the stream's start, as the probe's sound lead does (`stream_start`).
    let start_time = crate::render::probe::stream_start(&stream);
    set_discards(&mut input, Some(index), None);
    let time_base = f64::from(time_base);
    let until = until.unwrap_or(u64::MAX);
    // Position of the next resampled frame; unknown after a seek until the first frame's timestamp.
    let mut next: Option<u64> = Some(0);
    if from > 0 {
        let seek_s =
            from.saturating_sub(PRE_ROLL) as f64 / f64::from(RATE) + start_time as f64 * time_base;
        let ts = (seek_s * 1e6).round() as i64;
        input.seek(ts, ..ts).map_err(|_| DecodeError::CantSeek)?;
        next = None;
    }
    let mut frame = ffmpeg::frame::Audio::empty();
    let mut out = ResamplerOutput::new();
    let ch = usize::from(channels);
    // Hands the resampled block on, trimmed to [from, until). True: stop.
    let deliver = |at: u64, samples: &[f32], emit: &mut Emit| -> Result<bool, String> {
        let end = at + (samples.len() / ch) as u64;
        let (a, b) = (at.max(from), end.min(until));
        if a < b {
            let block = &samples[((a - at) as usize) * ch..((b - at) as usize) * ch];
            if !emit(a, block)? {
                return Ok(true);
            }
        }
        Ok(end >= until)
    };
    let mut done = false;
    loop {
        let packet = match next_packet(&mut input) {
            Ok(Some((s, packet))) if s == index => packet,
            Ok(Some(_)) => continue,
            Ok(None) => break,
            // Not the end of the file (an unplugged drive, a bad sector): the build fails, so its cache is never
            // finished short (`PcmStore` tries it again, a few times per opening).
            Err(e) => return Err(DecodeError::Unreadable(format!("Read / {e}"))),
        };
        decoder
            .send_packet(&packet)
            .map_err(|e| format!("Send Packet / {e}"))?;
        while decoder.receive_frame(&mut frame).is_ok() {
            let at = match next {
                Some(at) => at,
                None => {
                    let ts = frame
                        .timestamp()
                        .or_else(|| frame.pts())
                        .ok_or(DecodeError::CantSeek)?;
                    let at = ((ts.saturating_sub(start_time)) as f64 * time_base * f64::from(RATE))
                        .round()
                        .max(0.0) as u64;
                    if at > from {
                        return Err(DecodeError::CantSeek);
                    }
                    at
                }
            };
            out.run(&mut resampler, &frame)?;
            let samples = out.samples()?;
            next = Some(at + (samples.len() / ch) as u64);
            if deliver(at, samples, emit)? {
                done = true;
                break;
            }
        }
        if done {
            return Ok(());
        }
    }
    tracing::debug!(
        bytes_read = crate::media::scan::bytes_read(&input),
        "audio pass"
    );
    decoder.send_eof().map_err(|e| format!("Send EOF / {e}"))?;
    while decoder.receive_frame(&mut frame).is_ok() {
        let at = next.unwrap_or(from);
        out.run(&mut resampler, &frame)?;
        let samples = out.samples()?;
        next = Some(at + (samples.len() / ch) as u64);
        if deliver(at, samples, emit)? {
            return Ok(());
        }
    }
    // The resampler's tail.
    for _ in 0..64 {
        let Some(delay) = resampler.delay() else {
            break;
        };
        let capacity = (delay.output.max(1) as usize).saturating_add(16);
        let at = next.unwrap_or(from);
        let tail = out.prepare(&resampler, capacity);
        let remaining = resampler
            .flush(tail)
            .map_err(|e| format!("Flush Resampler / {e}"))?;
        let samples = out.samples()?;
        if samples.is_empty() {
            break;
        }
        next = Some(at + (samples.len() / ch) as u64);
        if deliver(at, samples, emit)? || remaining.is_none() {
            break;
        }
    }
    Ok(())
}

/// The loudest sample of each channel over the first `frames` (48 kHz) of `path`'s sound, 0–1, decoded now: for a file
/// whose sound cache (`peaks::loudest`) isn't built yet (chat's `centreSource`). Bounded by `frames`;
/// streaming, one decoded block at a time, nothing written or kept. None when it can't be decoded.
pub fn loudest(path: &Path, frames: u64) -> Option<Vec<f32>> {
    let channels = usize::from(probe(path).ok()?.channels);
    let mut peak = vec![0f32; channels];
    let mut emit = |_: u64, samples: &[f32]| -> Result<bool, String> {
        for frame in samples.chunks_exact(channels) {
            for (p, s) in peak.iter_mut().zip(frame) {
                *p = p.max(s.abs());
            }
        }
        Ok(true)
    };
    decode(path, 0, Some(frames), &mut emit).ok()?;
    Some(peak.into_iter().map(|p| p.min(1.0)).collect())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn the_loudest_sample_of_each_channel_is_read_only_as_far_as_asked() {
        // Two seconds of [0.5, silence], then a full-scale second channel past the bound.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one-sided.wav");
        let frames = 3 * RATE;
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let data = frames * 4;
        let header: [&[u8]; 12] = [
            b"RIFF",
            &(36 + data).to_le_bytes(),
            b"WAVEfmt ",
            &16u32.to_le_bytes(),
            &1u16.to_le_bytes(),
            &2u16.to_le_bytes(),
            &RATE.to_le_bytes(),
            &(RATE * 4).to_le_bytes(),
            &4u16.to_le_bytes(),
            &16u16.to_le_bytes(),
            b"data",
            &data.to_le_bytes(),
        ];
        header.iter().for_each(|part| out.write_all(part).unwrap());
        for n in 0..frames {
            let right: i16 = if n < 2 * RATE { 0 } else { 32_767 };
            out.write_all(&16_384i16.to_le_bytes()).unwrap();
            out.write_all(&right.to_le_bytes()).unwrap();
        }
        drop(out);
        let near = |got: Vec<f32>, want: [f32; 2]| {
            assert!(
                got.len() == 2 && got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01),
                "{got:?}"
            );
        };
        near(loudest(&path, u64::from(RATE)).unwrap(), [0.5, 0.0]);
        near(loudest(&path, u64::from(frames)).unwrap(), [0.5, 1.0]);
        assert_eq!(loudest(&dir.path().join("missing.wav"), 1), None);
    }

    /// A decode from a seek (its 2 s pre-roll thrown away, placed by its first frame's timestamp) gives exactly the
    /// samples a decode from the start gives from there.
    #[test]
    fn a_seek_decodes_the_same_samples_as_reading_from_the_start() {
        let path = crate::fixture_set::fixture_dir().join("longgop-20s-1080p.mp4");
        assert!(path.exists(), "run scripts/gen-engine-fixtures.sh");
        let from_frame = |from: u64| {
            let mut bits = Vec::new();
            decode(&path, from, None, &mut |_, block| {
                bits.extend(block.iter().map(|s| s.to_bits()));
                Ok(true)
            })
            .unwrap();
            bits
        };
        let whole = from_frame(0);
        // Mono: one sample a frame.
        let from = 7 * u64::from(RATE) + 1_234;
        let seeked = from_frame(from);
        assert!(!seeked.is_empty() && whole.len() > from as usize);
        assert!(seeked == whole[from as usize..], "from {from}");
    }
}
