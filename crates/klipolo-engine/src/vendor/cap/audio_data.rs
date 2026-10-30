// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/audio/src/audio_data.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: `crate::cast_bytes_to_f32_slice` replaced by a local copy of that function from Cap crates/audio/src/lib.rs; Cap's tests module and the test-support-only `from_raw_f32` removed (they need libc and Cap's test-support feature).

use ffmpeg::{
    ChannelLayout, codec as avcodec,
    format::{self as avformat},
    frame::Audio as FFAudio,
    software::resampling,
};
use std::{ops::Range, path::Path};

/// # Safety
/// The input slice must be aligned to the size of `f32`. (Copied from Cap `crates/audio/src/lib.rs`.)
unsafe fn cast_bytes_to_f32_slice(slice: &[u8]) -> &[f32] {
    unsafe {
        std::slice::from_raw_parts(
            slice.as_ptr() as *const f32,
            slice.len() / std::mem::size_of::<f32>(),
        )
    }
}

pub fn high_quality_resampler_options() -> ffmpeg::Dictionary<'static> {
    let mut options = ffmpeg::Dictionary::new();
    options.set("filter_size", "128");
    // FFmpeg parses option values with strtod, which honours LC_NUMERIC. GTK applies the
    // user's locale on Linux, so "0.97" fails with EINVAL under comma-decimal locales (#2359).
    options.set("cutoff", "97/100");
    options
}

// F32 Packed 48kHz audio
pub struct AudioData {
    samples: Vec<f32>,
    channels: u16,
    source_start_sample: usize,
    covered_source_end_sample: usize,
}

impl AudioData {
    pub const SAMPLE_FORMAT: avformat::Sample =
        avformat::Sample::F32(avformat::sample::Type::Packed);
    pub const SAMPLE_RATE: u32 = 48_000;

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::decode(path.as_ref(), None, true)
    }

    pub fn from_file_range(
        path: impl AsRef<Path>,
        source_start_sample: usize,
        source_end_sample: usize,
    ) -> Result<Self, String> {
        Self::decode(
            path.as_ref(),
            Some(source_start_sample..source_end_sample.max(source_start_sample)),
            true,
        )
    }

    fn decode(path: &Path, range: Option<Range<usize>>, allow_seek: bool) -> Result<Self, String> {
        let mut input_ctx =
            crate::media::open::input(&path).map_err(|e| format!("Input Open / {e}"))?;
        let input_stream = input_ctx
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or_else(|| "No Stream".to_string())?;

        let decoder_ctx = avcodec::Context::from_parameters(input_stream.parameters())
            .map_err(|e| format!("AudioData Parameters / {e}"))?;
        let mut decoder = decoder_ctx
            .decoder()
            .audio()
            .map_err(|e| format!("Set Parameters / {e}"))?;

        let source_channels = decoder.channels().max(1);
        if decoder.channel_layout().is_empty() {
            decoder.set_channel_layout(ChannelLayout::default(source_channels as i32));
        }
        let stream_time_base = input_stream.time_base();
        decoder.set_packet_time_base(stream_time_base);

        let target_channels = target_channels_for_source(source_channels);
        let target_channel_layout = ChannelLayout::default(target_channels as i32);
        let mut resampler = resampling::Context::get_with(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            AudioData::SAMPLE_FORMAT,
            target_channel_layout,
            AudioData::SAMPLE_RATE,
            high_quality_resampler_options(),
        )
        .map_err(|e| format!("Resampler / {e}"))?;

        let index = input_stream.index();
        let stream_start_time = input_stream.start_time();
        let stream_start_time = if stream_start_time == i64::MIN {
            0
        } else {
            stream_start_time
        };
        let stream_time_base = f64::from(stream_time_base);
        let source_start_sample = range.as_ref().map_or(0, |window| window.start);
        let covered_source_end_sample = range.as_ref().map_or(usize::MAX, |window| window.end);
        let mut sought = false;

        if allow_seek && source_start_sample > AudioData::SAMPLE_RATE as usize * 2 {
            let seek_sample =
                source_start_sample.saturating_sub(AudioData::SAMPLE_RATE as usize * 2);
            let seek_seconds = seek_sample as f64 / AudioData::SAMPLE_RATE as f64
                + stream_start_time as f64 * stream_time_base;
            let seek_timestamp = (seek_seconds * 1_000_000.0).round() as i64;
            sought = input_ctx.seek(seek_timestamp, ..seek_timestamp).is_ok();
        }

        let mut decoded_frame = ffmpeg::frame::Audio::empty();
        let mut resampler_output = ResamplerOutput::new();
        let mut samples = Vec::new();
        let mut resampled_samples = Vec::new();
        let mut next_source_sample = if sought { None } else { Some(0usize) };
        let mut complete = source_start_sample == covered_source_end_sample;

        'packets: for (stream, packet) in input_ctx.packets() {
            if complete {
                break;
            }
            if stream.index() != index {
                continue;
            }

            decoder
                .send_packet(&packet)
                .map_err(|e| format!("Send Packet / {e}"))?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if range.is_none() {
                    run_resampler(
                        &mut resampler,
                        &decoded_frame,
                        &mut resampler_output,
                        &mut samples,
                    )?;
                    continue;
                }

                if next_source_sample.is_none() {
                    let Some(timestamp) = decoded_frame.timestamp().or_else(|| decoded_frame.pts())
                    else {
                        return Self::decode(path, range, false);
                    };
                    let position = ((timestamp.saturating_sub(stream_start_time)) as f64
                        * stream_time_base
                        * AudioData::SAMPLE_RATE as f64)
                        .round()
                        .max(0.0) as usize;
                    if position > source_start_sample {
                        return Self::decode(path, range, false);
                    }
                    next_source_sample = Some(position);
                }

                resampled_samples.clear();
                run_resampler(
                    &mut resampler,
                    &decoded_frame,
                    &mut resampler_output,
                    &mut resampled_samples,
                )?;
                complete = append_sample_window(
                    &mut samples,
                    &resampled_samples,
                    &mut next_source_sample,
                    target_channels,
                    source_start_sample,
                    covered_source_end_sample,
                );
                if complete {
                    break 'packets;
                }
            }
        }

        if !complete {
            decoder.send_eof().map_err(|e| format!("Send EOF / {e}"))?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if range.is_none() {
                    run_resampler(
                        &mut resampler,
                        &decoded_frame,
                        &mut resampler_output,
                        &mut samples,
                    )?;
                    continue;
                }

                resampled_samples.clear();
                run_resampler(
                    &mut resampler,
                    &decoded_frame,
                    &mut resampler_output,
                    &mut resampled_samples,
                )?;
                complete = append_sample_window(
                    &mut samples,
                    &resampled_samples,
                    &mut next_source_sample,
                    target_channels,
                    source_start_sample,
                    covered_source_end_sample,
                );
                if complete {
                    break;
                }
            }

            if !complete {
                if range.is_some() {
                    resampled_samples.clear();
                    flush_resampler(
                        &mut resampler,
                        &mut resampler_output,
                        &mut resampled_samples,
                    )?;
                    append_sample_window(
                        &mut samples,
                        &resampled_samples,
                        &mut next_source_sample,
                        target_channels,
                        source_start_sample,
                        covered_source_end_sample,
                    );
                } else {
                    flush_resampler(&mut resampler, &mut resampler_output, &mut samples)?;
                }
            }
        }

        Ok(AudioData {
            samples,
            channels: target_channels,
            source_start_sample,
            covered_source_end_sample,
        })
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn samples(&self) -> &[f32] {
        self.samples.as_slice()
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn source_start_sample(&self) -> usize {
        self.source_start_sample
    }

    pub fn covers_source_range(
        &self,
        source_start_sample: usize,
        source_end_sample: usize,
    ) -> bool {
        source_start_sample >= self.source_start_sample
            && source_end_sample <= self.covered_source_end_sample
    }

    /// Interleaved 48 kHz samples starting at source sample 0. Used to
    /// assemble a track from decoded windows (zero elsewhere) when only parts
    /// of a long recording are needed.
    pub fn from_samples(samples: Vec<f32>, channels: u16) -> Self {
        Self {
            samples,
            channels,
            source_start_sample: 0,
            covered_source_end_sample: usize::MAX,
        }
    }
}

fn append_sample_window(
    samples: &mut Vec<f32>,
    resampled: &[f32],
    next_source_sample: &mut Option<usize>,
    channels: u16,
    source_start_sample: usize,
    covered_source_end_sample: usize,
) -> bool {
    let channels = channels as usize;
    let frame_start = next_source_sample.unwrap_or(0);
    let frame_end = frame_start.saturating_add(resampled.len() / channels);
    let overlap_start = frame_start.max(source_start_sample);
    let overlap_end = frame_end.min(covered_source_end_sample);

    if overlap_start < overlap_end {
        let start = (overlap_start - frame_start) * channels;
        let end = (overlap_end - frame_start) * channels;
        samples.extend_from_slice(&resampled[start..end]);
    }

    *next_source_sample = Some(frame_end);
    frame_end >= covered_source_end_sample
}

fn target_channels_for_source(channels: u16) -> u16 {
    if channels <= 1 { 1 } else { 2 }
}

pub(crate) struct ResamplerOutput {
    frame: FFAudio,
    capacity: usize,
}

impl ResamplerOutput {
    pub(crate) fn new() -> Self {
        Self {
            frame: FFAudio::empty(),
            capacity: 0,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        resampler: &resampling::Context,
        capacity: usize,
    ) -> &mut FFAudio {
        if capacity > self.capacity {
            let target = resampler.output();
            self.frame = FFAudio::new(target.format, capacity, target.channel_layout);
            self.capacity = capacity;
        }
        self.frame.set_samples(capacity);
        &mut self.frame
    }

    pub(crate) fn run(
        &mut self,
        resampler: &mut resampling::Context,
        decoded_frame: &FFAudio,
    ) -> Result<(), String> {
        let capacity = resample_capacity(resampler, decoded_frame.samples());
        let frame = self.prepare(resampler, capacity);
        resampler
            .run(decoded_frame, frame)
            .map_err(|error| format!("Run Resampler / {error}"))
            .map(|_| ())
    }

    pub(crate) fn samples(&self) -> Result<&[f32], String> {
        if self.frame.samples() == 0 {
            return Ok(&[]);
        }
        let byte_len = self
            .frame
            .samples()
            .saturating_mul(self.frame.channels() as usize)
            .saturating_mul(std::mem::size_of::<f32>());
        self.frame
            .data(0)
            .get(..byte_len)
            .map(|bytes| unsafe { cast_bytes_to_f32_slice(bytes) })
            .ok_or_else(|| "Resampled frame data shorter than expected".to_string())
    }
}

fn run_resampler(
    resampler: &mut resampling::Context,
    decoded_frame: &FFAudio,
    output: &mut ResamplerOutput,
    samples: &mut Vec<f32>,
) -> Result<(), String> {
    output.run(resampler, decoded_frame)?;
    samples.extend(output.samples()?);
    Ok(())
}

fn flush_resampler(
    resampler: &mut resampling::Context,
    output: &mut ResamplerOutput,
    samples: &mut Vec<f32>,
) -> Result<(), String> {
    for _ in 0..64 {
        let Some(delay) = resampler.delay() else {
            break;
        };
        let capacity = delay
            .output
            .max(1)
            .saturating_add(16)
            .min(i64::from(i32::MAX)) as usize;
        let resampled_frame = output.prepare(resampler, capacity);
        let remaining = resampler
            .flush(resampled_frame)
            .map_err(|e| format!("Flush Resampler / {e}"))?;

        let output_samples = resampled_frame.samples();
        append_resampled_frame(samples, resampled_frame)?;

        if remaining.is_none() || output_samples == 0 {
            break;
        }
    }

    Ok(())
}

fn resample_capacity(resampler: &resampling::Context, input_samples: usize) -> usize {
    let src_rate = resampler.input().rate.max(1) as u64;
    let dst_rate = resampler.output().rate.max(1) as u64;
    let pending_output_samples = resampler
        .delay()
        .map(|d| d.output.max(0) as u64)
        .unwrap_or(0);
    let resampled_from_input = (input_samples as u64)
        .saturating_mul(dst_rate)
        .div_ceil(src_rate);

    pending_output_samples
        .saturating_add(resampled_from_input)
        .saturating_add(16)
        .min(i32::MAX as u64) as usize
}

fn append_resampled_frame(samples: &mut Vec<f32>, frame: &FFAudio) -> Result<(), String> {
    if frame.samples() == 0 {
        return Ok(());
    }

    let byte_len = frame
        .samples()
        .saturating_mul(frame.channels() as usize)
        .saturating_mul(std::mem::size_of::<f32>());
    let data = frame
        .data(0)
        .get(..byte_len)
        .ok_or_else(|| "Resampled frame data shorter than expected".to_string())?;

    samples.extend(unsafe { cast_bytes_to_f32_slice(data) });
    Ok(())
}
