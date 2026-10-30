// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/audio/buffered_resampler.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted (`cap_media_info` → `super::super::super::media_info`); the unit tests removed.

use std::collections::VecDeque;

use super::super::super::media_info::AudioInfo;
use ffmpeg::software::resampling;

/// Consumes audio frames, resmaples them, buffers the results,
/// and allows retrieving new frames of any size.
/// When retrieving new frames via `get_frame`, silence will be accounted
/// for if the requested frame size is larger than the latest buffered frame,
/// ensuring that the resulting frame's PTS is always accurate.
pub struct BufferedResampler {
    resampler: ffmpeg::software::resampling::Context,
    buffer: VecDeque<(ffmpeg::frame::Audio, i64)>,
    sample_index: usize,
    // used to account for cases where pts is rounded down instead of up
    min_next_pts: Option<i64>,
}

impl BufferedResampler {
    pub fn new(from: AudioInfo, to: AudioInfo) -> Result<Self, ffmpeg::Error> {
        let resampler = ffmpeg::software::resampler(
            (from.sample_format, from.channel_layout(), from.sample_rate),
            (to.sample_format, to.channel_layout(), to.sample_rate),
        )?;

        Ok(Self {
            resampler,
            buffer: VecDeque::new(),
            sample_index: 0,
            min_next_pts: None,
        })
    }

    fn remaining_samples(&self) -> usize {
        let (mut pts, mut remaining_samples) = if let Some(front) = self.buffer.front() {
            (
                front.1 + front.0.samples() as i64,
                front.0.samples() - self.sample_index,
            )
        } else {
            return 0;
        };

        for buffer in self.buffer.iter().skip(1) {
            // Fill in gaps between buffered frames. Non-integer rate ratios
            // (44.1k -> 48k) can round consecutive pts to overlap by a sample,
            // making the difference negative; that is an overlap, not a gap.
            remaining_samples += (buffer.1 - pts).max(0) as usize;
            remaining_samples += buffer.0.samples();
            pts += buffer.0.samples() as i64;
        }

        remaining_samples
    }

    pub fn output(&self) -> resampling::context::Definition {
        *self.resampler.output()
    }

    pub fn input(&self) -> resampling::context::Definition {
        *self.resampler.input()
    }

    pub fn add_frame(&mut self, mut frame: ffmpeg::frame::Audio) {
        if let Some(min_next_pts) = self.min_next_pts
            && let Some(pts) = frame.pts()
        {
            frame.set_pts(Some(pts.max(min_next_pts)));
        }

        let pts = frame.pts().unwrap();

        let mut resampled_frame = ffmpeg::frame::Audio::empty();

        self.resampler.run(&frame, &mut resampled_frame).unwrap();

        let resampled_pts =
            (pts as f64 * (resampled_frame.rate() as f64 / frame.rate() as f64)) as i64;

        let mut next_pts = resampled_pts + resampled_frame.samples() as i64;

        self.buffer.push_back((resampled_frame, resampled_pts));

        while self.resampler.delay().is_some() {
            let mut resampled_frame = ffmpeg::frame::Audio::new(
                self.resampler.output().format,
                0,
                self.resampler.output().channel_layout,
            );
            self.resampler.flush(&mut resampled_frame).unwrap();
            let samples = resampled_frame.samples();
            if samples == 0 {
                break;
            }

            self.buffer.push_back((resampled_frame, next_pts));

            next_pts += samples as i64;
        }

        self.min_next_pts = Some(pts + frame.samples() as i64);
    }

    fn get_frame_inner(&mut self, samples: usize) -> Option<ffmpeg::frame::Audio> {
        let output = self.output();

        let mut out_frame =
            ffmpeg::frame::Audio::new(output.format, samples, output.channel_layout);

        let mut samples_already_written = 0;
        let mut current_pts = 0;

        if output.format.is_packed() {
            let bytes_per_sample =
                output.format.bytes() * output.channel_layout.channels() as usize;

            while let Some((frame, pts)) = self.buffer.pop_front() {
                if out_frame.pts().is_none() {
                    current_pts = pts + self.sample_index as i64;
                    out_frame.set_pts(Some(current_pts));
                }

                if pts >= current_pts + samples as i64 {
                    self.buffer.push_front((frame, pts));

                    let dest_range_start_samples = samples_already_written;
                    let dest_range_end_samples =
                        dest_range_start_samples + (samples - samples_already_written);
                    out_frame.data_mut(0)[dest_range_start_samples * bytes_per_sample
                        ..dest_range_end_samples * bytes_per_sample]
                        .fill(0);

                    break;
                }

                if current_pts < pts {
                    let silence_needed =
                        ((pts - current_pts) as usize).min(samples - samples_already_written);

                    out_frame.data_mut(0)[samples_already_written * bytes_per_sample
                        ..(samples_already_written + silence_needed) * bytes_per_sample]
                        .fill(0);

                    samples_already_written += silence_needed;
                    current_pts += silence_needed as i64;

                    if samples_already_written >= samples {
                        self.buffer.push_front((frame, pts));
                        break;
                    }
                }

                let sample_index = self.sample_index;

                let src_samples_remaining = frame.samples() - sample_index;

                let samples_to_write =
                    usize::min(src_samples_remaining, samples - samples_already_written);

                let dest_range_start = samples_already_written * bytes_per_sample;
                let dest_range_end = dest_range_start + samples_to_write * bytes_per_sample;

                let src_range_start = sample_index * bytes_per_sample;
                let src_range_end = src_range_start + samples_to_write * bytes_per_sample;

                out_frame.data_mut(0)[dest_range_start..dest_range_end]
                    .copy_from_slice(&frame.data(0)[src_range_start..src_range_end]);

                samples_already_written += samples_to_write;

                self.sample_index += samples_to_write;

                current_pts += samples_to_write as i64;

                if samples_to_write < src_samples_remaining {
                    self.buffer.push_front((frame, pts));
                    break;
                } else if samples_to_write >= src_samples_remaining {
                    self.sample_index -= frame.samples();
                }
            }
        } else {
            let channels = output.channel_layout.channels() as usize;
            let bytes_per_sample = output.format.bytes();

            while let Some((frame, pts)) = self.buffer.pop_front() {
                if out_frame.pts().is_none() {
                    current_pts = pts + self.sample_index as i64;
                    out_frame.set_pts(Some(current_pts));
                }

                if pts >= current_pts + samples as i64 {
                    self.buffer.push_front((frame, pts));

                    for i in 0..channels {
                        let dest_range_start_samples = samples_already_written;
                        let dest_range_end_samples =
                            dest_range_start_samples + (samples - samples_already_written);
                        out_frame.data_mut(i)[dest_range_start_samples * bytes_per_sample
                            ..dest_range_end_samples * bytes_per_sample]
                            .fill(0);
                    }

                    break;
                }

                if current_pts < pts {
                    let silence_needed =
                        ((pts - current_pts) as usize).min(samples - samples_already_written);

                    for i in 0..channels {
                        out_frame.data_mut(i)[samples_already_written * bytes_per_sample
                            ..(samples_already_written + silence_needed) * bytes_per_sample]
                            .fill(0);
                    }

                    samples_already_written += silence_needed;
                    current_pts += silence_needed as i64;

                    if samples_already_written >= samples {
                        self.buffer.push_front((frame, pts));
                        break;
                    }
                }

                let sample_index = self.sample_index;

                let src_samples_remaining = frame.samples() - sample_index;

                let samples_to_write =
                    usize::min(src_samples_remaining, samples - samples_already_written);

                let dest_range_start = samples_already_written * bytes_per_sample;
                let dest_range_end = dest_range_start + samples_to_write * bytes_per_sample;

                let src_range_start = sample_index * bytes_per_sample;
                let src_range_end = src_range_start + samples_to_write * bytes_per_sample;

                for i in 0..channels {
                    out_frame.data_mut(i)[dest_range_start..dest_range_end]
                        .copy_from_slice(&frame.data(i)[src_range_start..src_range_end]);
                }

                samples_already_written += samples_to_write;

                self.sample_index += samples_to_write;

                current_pts += samples_to_write as i64;

                if samples_to_write < src_samples_remaining {
                    self.buffer.push_front((frame, pts));
                    break;
                } else if samples_to_write >= src_samples_remaining {
                    self.sample_index -= frame.samples();
                }
            }
        }

        Some(out_frame)
    }

    pub fn get_frame(&mut self, samples: usize) -> Option<ffmpeg::frame::Audio> {
        if self.remaining_samples() < samples {
            return None;
        }

        self.get_frame_inner(samples)
    }

    pub fn flush(&mut self, max_samples: usize) -> Option<ffmpeg::frame::Audio> {
        let remaining_samples = self.remaining_samples();
        if remaining_samples == 0 {
            return None;
        }

        self.get_frame_inner(remaining_samples.min(max_samples))
    }
}
