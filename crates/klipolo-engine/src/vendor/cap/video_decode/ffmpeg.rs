// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/video-decode/src/ffmpeg.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: ffmpeg_hw_device path re-rooted; Windows D3D11 capability query removed (the non-Windows default is kept, uncfg'd); segmented/fragmented-MP4 input (DecoderInput::Segmented, DecoderPackets::Segmented, open_fragmented_input) and new_relocatable/new_relocatable_interruptible removed with every cap_enc_ffmpeg path; FFmpegDecoder::new no longer branches on path.is_dir(); tests (all needed cap_enc_ffmpeg/cap_media_info) removed. The frames iterator ticks the decoder's `DecodeProgress` (`set_progress`) for every packet sent and frame received, and in test and trial builds tells `decode_wait::hooks` of each decoder opened and frame decoded (the slow-software hook).

use crate::vendor::cap::ffmpeg_hw_device::{CodecContextExt, HwDevice};
use ffmpeg::{
    codec as avcodec,
    format::{self as avformat, context::input::PacketIter},
    frame as avframe,
    sys::{AVHWDeviceType, EAGAIN},
    util as avutil,
};
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing::*;

#[derive(Debug, Clone)]
pub struct HwDecoderCapabilities {
    pub max_width: u32,
    pub max_height: u32,
    pub supports_hw_decode: bool,
}

impl Default for HwDecoderCapabilities {
    fn default() -> Self {
        Self {
            max_width: 8192,
            max_height: 8192,
            supports_hw_decode: true,
        }
    }
}

static HW_CAPABILITIES: OnceLock<HwDecoderCapabilities> = OnceLock::new();

fn query_d3d11_video_decoder_capabilities() -> HwDecoderCapabilities {
    HwDecoderCapabilities::default()
}

pub fn get_hw_decoder_capabilities() -> &'static HwDecoderCapabilities {
    HW_CAPABILITIES.get_or_init(query_d3d11_video_decoder_capabilities)
}

fn configure_software_threading(decoder: &mut avcodec::decoder::Video, width: u32, height: u32) {
    let pixel_count = (width as u64) * (height as u64);
    let cpu_count = num_cpus::get();

    let thread_count = if pixel_count > 8294400 {
        0
    } else if pixel_count > 2073600 {
        cpu_count.clamp(2, 8) as i32
    } else {
        cpu_count.clamp(2, 6) as i32
    };

    let thread_type = ffmpeg::sys::FF_THREAD_FRAME | ffmpeg::sys::FF_THREAD_SLICE;

    unsafe {
        let codec_ctx = decoder.as_mut_ptr();
        if !codec_ctx.is_null() {
            (*codec_ctx).thread_count = thread_count;
            (*codec_ctx).thread_type = thread_type;
        }
    }

    info!(
        "Software decode configured: {width}x{height}, thread_count={}, thread_type=frame+slice, cpus={}",
        if thread_count == 0 {
            "auto".to_string()
        } else {
            thread_count.to_string()
        },
        cpu_count
    );
}

pub struct FFmpegDecoder {
    input: DecoderInput,
    decoder: avcodec::decoder::Video,
    stream_index: usize,
    hw_device: Option<HwDevice>,
    start_time: i64,
    /// Klipolo: ticked for every packet sent and frame received.
    progress: crate::decode_wait::DecodeProgress,
    /// Klipolo: the file and this decoder's number among the test hooks' (test and trial builds).
    #[cfg(any(test, feature = "trial"))]
    hook: (PathBuf, Option<usize>),
}

enum DecoderInput {
    File(avformat::context::Input),
}

impl DecoderInput {
    fn input(&self) -> &avformat::context::Input {
        match self {
            Self::File(input) => input,
        }
    }

    fn seek(&mut self, position: i64) -> Result<(), ffmpeg::Error> {
        match self {
            Self::File(input) => input.seek(position, ..position),
        }
    }

    fn packets(&mut self) -> DecoderPackets<'_> {
        match self {
            Self::File(input) => DecoderPackets::File(input.packets()),
        }
    }
}

enum DecoderPackets<'a> {
    File(PacketIter<'a>),
}

impl Iterator for DecoderPackets<'_> {
    type Item = Result<(usize, ffmpeg::Packet), ffmpeg::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::File(packets) => packets
                .next()
                .map(|(stream, packet)| Ok((stream.index(), packet))),
        }
    }
}

impl FFmpegDecoder {
    pub fn new(
        path: impl Into<PathBuf>,
        hw_device_type: Option<AVHWDeviceType>,
    ) -> Result<Self, String> {
        let path = path.into();

        let input = DecoderInput::File(
            crate::media::open::input(&path).map_err(|e| format!("open file / {e}"))?,
        );

        #[allow(unused_mut)]
        let mut decoder = Self::from_input(input, hw_device_type)?;
        #[cfg(any(test, feature = "trial"))]
        {
            decoder.hook = (
                path.clone(),
                crate::decode_wait::hooks::software_opened(&path),
            );
        }
        Ok(decoder)
    }

    /// Klipolo: the progress this decoder ticks.
    pub fn set_progress(&mut self, progress: crate::decode_wait::DecodeProgress) {
        self.progress = progress;
    }

    fn from_input(
        input: DecoderInput,
        hw_device_type: Option<AVHWDeviceType>,
    ) -> Result<Self, String> {
        let input_stream = input
            .input()
            .streams()
            .best(avutil::media::Type::Video)
            .ok_or_else(|| "no video stream".to_string())?;

        let start_time = input_stream.start_time();

        let stream_index = input_stream.index();

        let mut decoder = avcodec::Context::from_parameters(input_stream.parameters())
            .map_err(|e| format!("decoder context / {e}"))?
            .decoder()
            .video()
            .map_err(|e| format!("video decoder / {e}"))?;

        decoder.set_time_base(input_stream.time_base());

        let width = decoder.width();
        let height = decoder.height();

        let hw_caps = get_hw_decoder_capabilities();
        let exceeds_hw_limits =
            width > hw_caps.max_width || height > hw_caps.max_height || !hw_caps.supports_hw_decode;

        let hw_device = hw_device_type.and_then(|hw_device_type| {
            if exceeds_hw_limits {
                warn!(
                    "Video dimensions {width}x{height} exceed hardware decoder limits ({}x{}), using software decode",
                    hw_caps.max_width, hw_caps.max_height
                );
                configure_software_threading(&mut decoder, width, height);
                None
            } else {
                match decoder.try_use_hw_device(hw_device_type) {
                    Ok(device) => {
                        info!(
                            "Using hardware acceleration for {width}x{height} video (device: {:?})",
                            hw_device_type
                        );
                        Some(device)
                    }
                    Err(error) => {
                        warn!("Failed to enable hardware decoder: {error:?}, falling back to optimized software decode");
                        configure_software_threading(&mut decoder, width, height);
                        None
                    }
                }
            }
        });

        if hw_device.is_none() && hw_device_type.is_none() {
            configure_software_threading(&mut decoder, width, height);
        }

        Ok(FFmpegDecoder {
            input,
            decoder,
            stream_index,
            hw_device,
            start_time,
            progress: Default::default(),
            #[cfg(any(test, feature = "trial"))]
            hook: (PathBuf::new(), None),
        })
    }

    pub fn reset(&mut self, requested_time: f32) -> Result<(), ffmpeg::Error> {
        use ffmpeg::rescale;
        let timestamp_us = (requested_time * 1_000_000.0) as i64;
        let position = rescale::Rescale::rescale(&timestamp_us, (1, 1_000_000), rescale::TIME_BASE);

        self.decoder.flush();
        self.input.seek(position)
    }

    pub fn frames(&mut self) -> FramesIter<'_> {
        FramesIter {
            packets: self.input.packets(),
            decoder: &mut self.decoder,
            stream_index: self.stream_index,
            hw_device: self.hw_device.as_mut(),
            progress: &self.progress,
            #[cfg(any(test, feature = "trial"))]
            hook: (&self.hook, self.start_time),
        }
    }

    pub fn decoder(&self) -> &avcodec::decoder::Video {
        &self.decoder
    }

    pub fn start_time(&self) -> i64 {
        self.start_time
    }

    pub fn is_hardware_accelerated(&self) -> bool {
        self.hw_device.is_some()
    }
}

unsafe impl Send for FFmpegDecoder {}

pub struct FramesIter<'a> {
    decoder: &'a mut avcodec::decoder::Video,
    packets: DecoderPackets<'a>,
    stream_index: usize,
    hw_device: Option<&'a mut HwDevice>,
    progress: &'a crate::decode_wait::DecodeProgress,
    #[cfg(any(test, feature = "trial"))]
    hook: (&'a (PathBuf, Option<usize>), i64),
}

impl FramesIter<'_> {
    pub fn decoder(&self) -> &avcodec::decoder::Video {
        self.decoder
    }
}

impl<'a> Iterator for FramesIter<'a> {
    type Item = Result<avframe::Video, avutil::error::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut frame = avframe::Video::empty();

        loop {
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    self.progress.tick();
                    #[cfg(any(test, feature = "trial"))]
                    {
                        let ((path, decoder), start) = self.hook;
                        let tb = self.decoder.time_base();
                        let secs = (frame.pts().unwrap_or(0) - start) as f64
                            * f64::from(tb.numerator())
                            / f64::from(tb.denominator().max(1));
                        crate::decode_wait::hooks::software_frame(path, *decoder, secs);
                    }
                    return match &self.hw_device {
                        Some(hw_device) => {
                            let hw_result = hw_device.get_hwframe(&frame);
                            Some(Ok(hw_result.unwrap_or(frame)))
                        }
                        None => Some(Ok(frame)),
                    };
                }
                Err(ffmpeg::Error::Eof) => {
                    return None;
                }
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => {}
                Err(e) => return Some(Err(e)),
            }

            let (stream_index, packet) = match self.packets.next()? {
                Ok(packet) => packet,
                Err(error) => return Some(Err(error)),
            };

            if stream_index != self.stream_index {
                continue;
            };

            self.progress.tick();
            match self.decoder.send_packet(&packet) {
                Ok(_) => {}
                Err(ffmpeg::Error::Eof) => return None,
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => {}
                Err(e) => return Some(Err(e)),
            }
        }
    }
}
