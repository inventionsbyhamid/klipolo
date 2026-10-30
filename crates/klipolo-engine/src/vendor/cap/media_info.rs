// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/media-info/src/lib.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: the cpal stream-config constructors (`from_stream_config*`, `ffmpeg_sample_format_for`) and the unit tests removed (export builds its audio info from the mix: 48 kHz f32 stereo).

use ffmpeg::frame;
pub use ffmpeg::{
    format::{
        pixel::Pixel,
        sample::{Sample, Type},
    },
    util::{channel_layout::ChannelLayout, rational::Rational as FFRational},
};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct AudioInfo {
    pub sample_format: Sample,
    pub sample_rate: u32,
    pub channels: usize,
    pub time_base: FFRational,
    pub buffer_size: u32,
    pub is_wireless_transport: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AudioInfoError {
    #[error("Unsupported number of channels: {0}")]
    ChannelLayout(u16),
}

impl AudioInfo {
    /// Maximum number of audio channels supported by FFmpeg channel layouts.
    /// Matches the highest channel count in `channel_layout_raw` (7.1 surround = 8 channels).
    /// Note: Actual device channel counts may exceed this; we store the real count
    /// but clamp to this value when creating FFmpeg frames.
    pub const MAX_AUDIO_CHANNELS: u16 = 8;

    pub const fn new(
        sample_format: Sample,
        sample_rate: u32,
        channel_count: u16,
    ) -> Result<Self, AudioInfoError> {
        if Self::channel_layout_raw(channel_count).is_none() {
            return Err(AudioInfoError::ChannelLayout(channel_count));
        }

        Ok(Self {
            sample_format,
            sample_rate,
            channels: channel_count as usize,
            time_base: FFRational(1, 1_000_000),
            buffer_size: 1024,
            is_wireless_transport: false,
        })
    }

    pub const fn new_raw(sample_format: Sample, sample_rate: u32, channel_count: u16) -> Self {
        Self {
            sample_format,
            sample_rate,
            channels: channel_count as usize,
            time_base: FFRational(1, 1_000_000),
            buffer_size: 1024,
            is_wireless_transport: false,
        }
    }

    pub fn from_decoder(decoder: &ffmpeg::codec::decoder::Audio) -> Result<Self, AudioInfoError> {
        Self::channel_layout_raw(decoder.channels())
            .ok_or(AudioInfoError::ChannelLayout(decoder.channels()))?;

        Ok(Self {
            sample_format: decoder.format(),
            sample_rate: decoder.rate(),
            channels: usize::from(decoder.channels()),
            time_base: decoder.time_base(),
            buffer_size: decoder.frame_size(),
            is_wireless_transport: false,
        })
    }

    const fn channel_layout_raw(channels: u16) -> Option<ChannelLayout> {
        Some(match channels {
            1 => ChannelLayout::MONO,
            2 => ChannelLayout::STEREO,
            3 => ChannelLayout::SURROUND,
            4 => ChannelLayout::QUAD,
            5 => ChannelLayout::_5POINT0,
            6 => ChannelLayout::_5POINT1,
            7 => ChannelLayout::_6POINT1,
            8 => ChannelLayout::_7POINT1,
            _ => return None,
        })
    }

    pub fn channel_layout(&self) -> ChannelLayout {
        // Clamp channels to supported range and return appropriate layout.
        // This prevents panics when audio devices report unusual channel counts
        // (e.g., 0 channels or more than 8 channels).
        let clamped_channels = (self.channels as u16).clamp(1, 8);
        Self::channel_layout_raw(clamped_channels).unwrap_or(ChannelLayout::STEREO)
    }

    /// Channel count a packed frame from [`Self::wrap_frame_with_max_channels`] carries
    /// for `max_channels`. The single source of truth for the wrapped count so a
    /// resampler reading those frames cannot disagree on the channel count.
    pub fn wrapped_frame_channels(&self, max_channels: usize) -> usize {
        self.channels
            .max(1)
            .min(max_channels.max(1))
            .min(Self::MAX_AUDIO_CHANNELS as usize)
    }

    /// Channel layout a packed frame from [`Self::wrap_frame_with_max_channels`] carries.
    ///
    /// A resampler that consumes those frames MUST be configured with this exact
    /// layout. `swr_convert_frame` revalidates every input frame's channel layout
    /// against the context and silently rejects it (→ the whole frame is dropped →
    /// silent track) when the masks differ. The named [`Self::channel_layout`] masks
    /// diverge from `ChannelLayout::default` for 3/4/5/6-channel sources, so the two
    /// must never be mixed across a frame builder and the resampler reading it.
    pub fn wrapped_frame_layout(&self, max_channels: usize) -> ChannelLayout {
        ChannelLayout::default(self.wrapped_frame_channels(max_channels) as i32)
    }

    /// Channel layout for an un-clamped packed frame of this info's channel count,
    /// matching frames built with `ChannelLayout::default(channels)` (e.g.
    /// `scap_ffmpeg::DataExt::as_ffmpeg`). See [`Self::wrapped_frame_layout`] for why
    /// a resampler's input layout must match its frame builder's exactly.
    pub fn packed_channel_layout(&self) -> ChannelLayout {
        ChannelLayout::default(self.channels.max(1) as i32)
    }

    pub fn sample_size(&self) -> usize {
        self.sample_format.bytes()
    }

    pub const fn rate(&self) -> i32 {
        self.sample_rate as i32
    }

    pub fn empty_frame(&self, sample_count: usize) -> frame::Audio {
        let mut frame = frame::Audio::new(self.sample_format, sample_count, self.channel_layout());
        frame.set_rate(self.sample_rate);

        frame
    }

    /// Always expects packed input data
    pub fn wrap_frame_with_max_channels(
        &self,
        packed_data: &[u8],
        max_channels: usize,
    ) -> frame::Audio {
        // Use actual channel count for parsing input data (at least 1 to avoid div by zero)
        let input_channels = self.channels.max(1);
        // Clamp output channels to both max_channels and MAX_AUDIO_CHANNELS for FFmpeg
        // compatibility. Shared with `wrapped_frame_layout` so a resampler configured
        // for these frames is guaranteed the same channel count + layout.
        let out_channels = self.wrapped_frame_channels(max_channels);

        let sample_size = self.sample_size();
        let packed_sample_size = sample_size * input_channels;
        let samples = packed_data.len() / packed_sample_size;

        let mut frame = frame::Audio::new(
            self.sample_format,
            samples,
            ChannelLayout::default(out_channels as i32),
        );
        frame.set_rate(self.sample_rate);

        if input_channels == 1 || (frame.is_packed() && input_channels <= out_channels) {
            // frame is allocated with parameters derived from packed_data, so this is safe
            frame.data_mut(0)[0..packed_data.len()].copy_from_slice(packed_data);
        } else if frame.is_packed() && input_channels > out_channels {
            for (chunk_index, packed_chunk) in packed_data.chunks(packed_sample_size).enumerate() {
                let start = chunk_index * sample_size * out_channels;

                let copy_len = sample_size * out_channels;

                if let (Some(chunk_slice), Some(frame_slice)) = (
                    packed_chunk.get(0..copy_len),
                    frame.data_mut(0).get_mut(start..start + copy_len),
                ) {
                    frame_slice.copy_from_slice(chunk_slice);
                }
            }
        } else {
            for (chunk_index, packed_chunk) in packed_data.chunks(packed_sample_size).enumerate() {
                let start = chunk_index * sample_size;

                for channel in 0..out_channels {
                    let channel_start = channel * sample_size;
                    let channel_end = channel_start + sample_size;
                    if let (Some(chunk_slice), Some(frame_slice)) = (
                        packed_chunk.get(channel_start..channel_end),
                        frame.data_mut(channel).get_mut(start..start + sample_size),
                    ) {
                        frame_slice.copy_from_slice(chunk_slice);
                    }
                }
            }
        }

        frame
    }

    /// Always expects packed input data
    pub fn wrap_frame(&self, data: &[u8]) -> frame::Audio {
        self.wrap_frame_with_max_channels(data, self.channels)
    }

    pub fn with_wireless_transport(mut self, is_wireless: bool) -> Self {
        self.is_wireless_transport = is_wireless;
        self
    }

    pub fn with_max_channels(&self, channels: u16) -> Self {
        let mut this = *self;
        this.channels = this.channels.min(channels as usize);
        this
    }

    pub fn with_sample_rate(&self, rate: u32) -> Self {
        let mut this = *self;
        this.sample_rate = rate;
        this
    }

    pub fn with_sample_format(&self, format: Sample) -> Self {
        let mut this = *self;
        this.sample_format = format;
        this
    }

    pub fn with_channels(&self, channels: usize) -> Self {
        let mut this = *self;
        this.channels = channels;
        this
    }

    pub fn matches_format(&self, other: &Self) -> bool {
        self.sample_rate == other.sample_rate
            && self.channels == other.channels
            && self.sample_format == other.sample_format
    }

    /// Returns a version of this AudioInfo with channels clamped for FFmpeg compatibility.
    /// FFmpeg channel layouts only support up to 8 channels (7.1 surround).
    pub fn for_ffmpeg_output(&self) -> Self {
        self.with_max_channels(Self::MAX_AUDIO_CHANNELS)
    }
}

pub enum RawVideoFormat {
    Bgra,
    Mjpeg,
    Uyvy,
    RawRgb,
    Nv12,
    Nv21,
    Gray,
    Gray16,
    Yuyv422,
    Yuv420p,
    Rgba,
    Rgb565,
    P010,
}

#[derive(Debug, Copy, Clone)]
pub struct VideoInfo {
    pub pixel_format: Pixel,
    pub width: u32,
    pub height: u32,
    pub time_base: FFRational,
    pub frame_rate: FFRational,
}

impl VideoInfo {
    pub fn from_raw(format: RawVideoFormat, width: u32, height: u32, fps: u32) -> Self {
        Self {
            pixel_format: match format {
                RawVideoFormat::Bgra => Pixel::BGRA,
                RawVideoFormat::Mjpeg => Pixel::YUVJ422P,
                RawVideoFormat::Uyvy => Pixel::UYVY422,
                RawVideoFormat::RawRgb => Pixel::RGB24,
                RawVideoFormat::Nv12 => Pixel::NV12,
                RawVideoFormat::Nv21 => Pixel::NV21,
                RawVideoFormat::Gray => Pixel::GRAY8,
                RawVideoFormat::Gray16 => Pixel::GRAY16LE,
                RawVideoFormat::Yuyv422 => Pixel::YUYV422,
                RawVideoFormat::Yuv420p => Pixel::YUV420P,
                RawVideoFormat::Rgba => Pixel::RGBA,
                RawVideoFormat::Rgb565 => Pixel::RGB565LE,
                RawVideoFormat::P010 => Pixel::P010LE,
            },
            width,
            height,
            time_base: FFRational(1, 1_000_000),
            frame_rate: FFRational(fps as i32, 1),
        }
    }

    pub fn from_raw_ffmpeg(pixel_format: Pixel, width: u32, height: u32, fps: u32) -> Self {
        Self {
            pixel_format,
            width,
            height,
            time_base: FFRational(1, 1_000_000),
            frame_rate: FFRational(fps as i32, 1),
        }
    }

    pub fn fps(&self) -> u32 {
        self.frame_rate.0 as u32
    }

    pub fn scaled(&self, width: u32, fps: u32) -> Self {
        let (width, height) = match self.width <= width {
            true => (self.width, self.height),
            false => {
                let new_width = width & !1;
                let new_height = (((new_width as f32) * (self.height as f32) / (self.width as f32))
                    .round() as u32)
                    & !1;
                (new_width, new_height)
            }
        };

        Self {
            pixel_format: self.pixel_format,
            width,
            height,
            time_base: self.time_base,
            frame_rate: FFRational(fps.try_into().unwrap(), 1),
        }
    }

    pub fn wrap_frame(&self, data: &[u8], timestamp: i64, stride: usize) -> frame::Video {
        let mut frame = frame::Video::new(self.pixel_format, self.width, self.height);
        frame.set_pts(Some(timestamp));

        let frame_stride = frame.stride(0);
        let frame_height = self.height as usize;

        // Ensure we don't try to copy more data than we have
        if frame.stride(0) == self.width as usize {
            let copy_len = std::cmp::min(data.len(), frame.data_mut(0).len());
            frame.data_mut(0)[0..copy_len].copy_from_slice(&data[0..copy_len]);
        } else {
            for line in 0..frame_height {
                if line * stride >= data.len() {
                    break; // Stop if we run out of source data
                }

                let src_start = line * stride;
                let src_end = std::cmp::min(src_start + frame_stride, data.len());
                if src_end <= src_start {
                    break; // Stop if we can't get any more source data
                }

                let dst_start = line * frame_stride;
                let dst_end = dst_start + (src_end - src_start);

                // Only copy if we have enough destination space
                if dst_end <= frame.data_mut(0).len() {
                    frame.data_mut(0)[dst_start..dst_end]
                        .copy_from_slice(&data[src_start..src_end]);
                }
            }
        }

        frame
    }
}

pub fn ensure_even(value: u32) -> u32 {
    let adjusted = value - (value % 2);
    if adjusted == 0 { 2 } else { adjusted }
}
