// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/video/h264.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted (`cap_media_info` → `super::super::super::media_info`); no libx264 and no other vendors: the encoder-priority lists, GPU-vendor selection (`cap_frame_converter`), the `CAP_EXPORT_FORCE_SOFTWARE_ENCODER` path, the hardware self-test, the realtime-capability estimates, presets, bpp bitrates, CRF and `build_standalone` (with `h264_packet.rs`) removed, as are Cap's unit tests; macOS VideoToolbox only, generalised to the export codecs (`VtCodec`: `h264_videotoolbox` High, `hevc_videotoolbox` Main 10 tagged `hvc1`, `prores_videotoolbox` HQ); export options: `realtime=false`, the codec's profile, `allow_sw=0` in hardware mode (ProRes always allows software: Intel has no ProRes engine); builder flags `with_bitrate` (bit_rate and a 1 s max_rate peak), `with_max_bitrate`, `with_gop`, `with_b_frames` and `with_software_only` (`require_sw=1`, `allow_sw=1`, VideoToolbox's software encoder, LGPL-clean); the zero-copy input takes the hardware frames' software format from the input (NV12, P010 or P210); `queue_frame` takes `&mut frame::Video` (no per-frame move); a format the encoder doesn't take converts to the first software format it lists; closed GOPs (`AV_CODEC_FLAG_CLOSED_GOP`: no reference across a keyframe, so export segments join); `open` and `open_videotoolbox_hw` build the encoder without a stream (`stream_template` adds one to each segment file, `build*` still add theirs), `encode_to` takes the frame's pts and a forced keyframe (`pict_type` I, which VideoToolbox turns into `kVTEncodeFrameOptionKey_ForceKeyFrame`) and hands packets to a sink, `flush_to`, `extradata`, `time_base`; `unsafe impl Send` moved here from the removed `mux/mp4.rs`; `with_encoder_name` opens another FFmpeg encoder for the codec (the platform's candidate list); `with_options` sets extra AVOptions on the encoder before it opens (editing copies' `power_efficient=1`), and one the encoder doesn't know or rejects is logged and ignored, never fatal; `wrap_videotoolbox_pixel_buffer_then` (calls back once VideoToolbox lets the wrapped buffer go).

use std::{thread, time::Duration};

use super::super::super::media_info::{Pixel, VideoInfo};
use ffmpeg::{
    Dictionary, Packet,
    codec::{codec::Codec, context, encoder},
    color,
    format::{self},
    frame,
    threading::Config,
};
use tracing::{debug, error, info, trace};

use super::super::base::EncoderBase;
#[cfg(target_os = "macos")]
use super::videotoolbox_hw::VideoToolboxHwFrames;
use std::sync::Arc;

fn is_420(format: ffmpeg::format::Pixel) -> bool {
    format
        .descriptor()
        .map(|desc| desc.log2_chroma_w() == 1 && desc.log2_chroma_h() == 1)
        .unwrap_or(false)
}

/// Klipolo: the VideoToolbox encoders export uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VtCodec {
    H264High,
    HevcMain10,
    ProRes422Hq,
}

impl VtCodec {
    pub fn encoder_name(self) -> &'static str {
        match self {
            Self::H264High => "h264_videotoolbox",
            Self::HevcMain10 => "hevc_videotoolbox",
            Self::ProRes422Hq => "prores_videotoolbox",
        }
    }

    fn profile(self) -> &'static str {
        match self {
            Self::H264High => "high",
            Self::HevcMain10 => "main10",
            Self::ProRes422Hq => "hq",
        }
    }
}

/// Klipolo: the colour tags an encoder writes (a reversed copy keeps its source's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorTags {
    pub range: color::Range,
    pub space: color::Space,
    pub primaries: color::Primaries,
    pub transfer: color::TransferCharacteristic,
}

#[derive(Clone)]
pub struct H264EncoderBuilder {
    input_config: VideoInfo,
    codec: VtCodec,
    encoder_name: Option<&'static str>,
    bitrate: Option<usize>,
    max_bitrate: Option<usize>,
    gop: Option<u32>,
    b_frames: usize,
    software_only: bool,
    hardware_only: bool,
    /// Klipolo: the colour tags written (`with_color`); None writes BT.709 limited range.
    color: Option<ColorTags>,
    /// Klipolo: extra AVOptions (name, value), set on the encoder before it opens (`with_options`).
    extra_options: Vec<(String, String)>,
}

#[derive(thiserror::Error, Debug)]
pub enum H264EncoderError {
    #[error("{0:?}")]
    FFmpeg(#[from] ffmpeg::Error),
    #[error("Codec not found")]
    CodecNotFound,
    #[error("Failed to configure RGB conversion for BT.709 limited-range video: {0}")]
    ColorConversion(ffmpeg::Error),
    #[error("Pixel format {0:?} not supported")]
    PixFmtNotSupported(Pixel),
    #[error("Invalid output dimensions {width}x{height}; expected non-zero even width and height")]
    InvalidOutputDimensions { width: u32, height: u32 },
    #[error("VideoToolbox hardware input unavailable: {0}")]
    VideoToolboxHwInput(String),
}

impl H264EncoderBuilder {
    pub fn new(input_config: VideoInfo) -> Self {
        Self {
            input_config,
            codec: VtCodec::H264High,
            encoder_name: None,
            bitrate: None,
            max_bitrate: None,
            gop: None,
            b_frames: 0,
            software_only: false,
            hardware_only: false,
            color: None,
            extra_options: Vec::new(),
        }
    }

    /// Klipolo: the colour tags to write instead of BT.709 limited range.
    pub fn with_color(mut self, tags: ColorTags) -> Self {
        self.color = Some(tags);
        self
    }

    /// Klipolo: which VideoToolbox encoder (H.264 by default).
    pub fn with_codec(mut self, codec: VtCodec) -> Self {
        self.codec = codec;
        self
    }

    /// Klipolo: the FFmpeg encoder that runs `codec` (the platform's candidate, `platform::encode`); VideoToolbox's
    /// by default.
    pub fn with_encoder_name(mut self, name: &'static str) -> Self {
        self.encoder_name = Some(name);
        self
    }

    fn ffmpeg_name(&self) -> &'static str {
        self.encoder_name.unwrap_or(self.codec.encoder_name())
    }

    /// Klipolo: the average bitrate, with a peak of 1.5× over one second unless `with_max_bitrate` says otherwise.
    pub fn with_bitrate(mut self, bps: u32) -> Self {
        self.bitrate = (bps > 0).then_some(bps as usize);
        self
    }

    pub fn with_max_bitrate(mut self, bps: u32) -> Self {
        self.max_bitrate = (bps > 0).then_some(bps as usize);
        self
    }

    /// Klipolo: frames between keyframes (FFmpeg's default is 12, a keyframe every 0.4 s at 30 fps).
    pub fn with_gop(mut self, frames: u32) -> Self {
        self.gop = Some(frames.max(1));
        self
    }

    pub fn with_b_frames(mut self, frames: u32) -> Self {
        self.b_frames = frames as usize;
        self
    }

    /// Klipolo: VideoToolbox's software encoder (`require_sw=1`; SoftwareSafe mode).
    pub fn with_software_only(mut self, software: bool) -> Self {
        self.software_only = software;
        self
    }

    /// Klipolo: refuse the software encoder even for ProRes (probing for a ProRes engine).
    pub fn with_hardware_only(mut self, hardware: bool) -> Self {
        self.hardware_only = hardware;
        self
    }

    /// Klipolo: extra AVOptions for the encoder (`platform::encode::background_options`). Each is set on the encoder
    /// before it opens; one it doesn't know, or whose value it rejects, is logged and ignored.
    pub fn with_options(mut self, options: &[(&str, &str)]) -> Self {
        self.extra_options.extend(
            options
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        );
        self
    }

    /// Klipolo: sets `extra_options` on the encoder's private options (its defaults are already in place: the context was
    /// made with the codec), so `open` never fails over one of them.
    fn apply_extra_options(&self, encoder: &mut encoder::video::Video) {
        for (name, value) in &self.extra_options {
            let (Ok(c_name), Ok(c_value)) = (
                std::ffi::CString::new(name.as_str()),
                std::ffi::CString::new(value.as_str()),
            ) else {
                continue;
            };
            // SAFETY: the context is open for configuration (not yet opened) and owns its private data; `av_opt_set`
            // copies the strings and fails cleanly for an unknown name or an invalid value.
            let ret = unsafe {
                let priv_data = (*encoder.as_mut_ptr()).priv_data;
                if priv_data.is_null() {
                    -1
                } else {
                    ffmpeg::ffi::av_opt_set(priv_data, c_name.as_ptr(), c_value.as_ptr(), 0)
                }
            };
            if ret < 0 {
                tracing::warn!(
                    encoder = %self.ffmpeg_name(),
                    option = %name,
                    value = %value,
                    "the encoder doesn't take this option; ignored"
                );
            }
        }
    }

    fn options(&self) -> Dictionary<'static> {
        let mut options = Dictionary::new();
        options.set("realtime", "false");
        options.set("profile", self.codec.profile());
        if self.software_only {
            options.set("require_sw", "1");
            options.set("allow_sw", "1");
        } else if self.codec == VtCodec::ProRes422Hq && !self.hardware_only {
            options.set("allow_sw", "1");
        } else {
            options.set("allow_sw", "0");
        }
        options
    }

    fn configure(&self, encoder: &mut encoder::video::Video) {
        match self.color {
            // Klipolo: a source's own tags.
            Some(tags) => {
                encoder.set_colorspace(tags.space);
                encoder.set_color_range(tags.range);
                // SAFETY: plain fields of the context, set before it opens.
                unsafe {
                    (*encoder.as_mut_ptr()).color_primaries = tags.primaries.into();
                    (*encoder.as_mut_ptr()).color_trc = tags.transfer.into();
                }
            }
            None => {
                encoder.set_colorspace(color::Space::BT709);
                encoder.set_color_range(color::Range::MPEG);
                unsafe {
                    (*encoder.as_mut_ptr()).color_primaries =
                        ffmpeg::ffi::AVColorPrimaries::AVCOL_PRI_BT709;
                    (*encoder.as_mut_ptr()).color_trc =
                        ffmpeg::ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
                }
            }
        }
        if let Some(bitrate) = self.bitrate {
            encoder.set_bit_rate(bitrate);
            encoder.set_max_bit_rate(self.max_bitrate.unwrap_or(bitrate * 3 / 2));
        }
        if let Some(gop) = self.gop {
            encoder.set_gop(gop);
        }
        encoder.set_max_b_frames(self.b_frames);
        // Klipolo: closed GOPs (VideoToolbox's AllowOpenGOP off): export segments are joined at keyframes.
        unsafe {
            (*encoder.as_mut_ptr()).flags |= ffmpeg::ffi::AV_CODEC_FLAG_CLOSED_GOP as i32;
        }
        self.apply_extra_options(encoder);
    }

    /// Software-input frames in the input's pixel format (NV12, P010 or a 4:2:2 10-bit format); a format the encoder
    /// doesn't take is converted with swscale (BT.709).
    pub fn build(
        self,
        output: &mut format::context::Output,
    ) -> Result<H264Encoder, H264EncoderError> {
        let mut encoder = self.open()?;
        encoder.attach(output)?;
        Ok(encoder)
    }

    /// Klipolo: `build` without adding a stream (see `H264Encoder::add_stream_to`).
    pub fn open(self) -> Result<H264Encoder, H264EncoderError> {
        let codec =
            encoder::find_by_name(self.ffmpeg_name()).ok_or(H264EncoderError::CodecNotFound)?;
        let input_config = self.input_config;
        let OpenedVideoEncoder {
            encoder,
            converter,
            converted_frame_pool,
            output_format,
            output_width,
            output_height,
            input_format,
            input_width,
            input_height,
        } = open_video_encoder_inner(&self, codec, self.options(), &input_config)?;

        info!(
            encoder = %codec.name(),
            width = output_width,
            height = output_height,
            software = self.software_only,
            "Selected VideoToolbox encoder"
        );

        Ok(H264Encoder {
            base: EncoderBase::new(0),
            codec,
            kind: self.codec,
            frame_rate: input_config.frame_rate,
            encoder,
            converter,
            output_format,
            output_width,
            output_height,
            input_format,
            input_width,
            input_height,
            converted_frame_pool,
            #[cfg(target_os = "macos")]
            videotoolbox_hw: None,
        })
    }

    /// Builds an encoder that consumes IOSurface-backed CVPixelBuffers directly (`AV_PIX_FMT_VIDEOTOOLBOX`),
    /// skipping the per-frame CPU plane copy a software-input encoder requires. Frames are queued through
    /// [`H264Encoder::wrap_videotoolbox_pixel_buffer`]. Klipolo: the input's pixel format (NV12, P010 or P210) is the
    /// buffers' software format.
    #[cfg(target_os = "macos")]
    pub fn build_videotoolbox_hw_input(
        self,
        output: &mut format::context::Output,
    ) -> Result<H264Encoder, H264EncoderError> {
        let mut encoder = self.open_videotoolbox_hw()?;
        encoder.attach(output)?;
        Ok(encoder)
    }

    /// Klipolo: `build_videotoolbox_hw_input` without adding a stream (see `H264Encoder::add_stream_to`).
    #[cfg(target_os = "macos")]
    pub fn open_videotoolbox_hw(self) -> Result<H264Encoder, H264EncoderError> {
        let input_config = self.input_config;
        let output_width = input_config.width;
        let output_height = input_config.height;
        if !output_width.is_multiple_of(2) || !output_height.is_multiple_of(2) {
            return Err(H264EncoderError::InvalidOutputDimensions {
                width: output_width,
                height: output_height,
            });
        }

        let codec =
            encoder::find_by_name(self.ffmpeg_name()).ok_or(H264EncoderError::CodecNotFound)?;

        let hw_frames =
            VideoToolboxHwFrames::new(output_width, output_height, input_config.pixel_format)
                .map_err(|e| H264EncoderError::VideoToolboxHwInput(e.to_string()))?;

        let thread_count = thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(1);

        let encoder = {
            let mut encoder_ctx = context::Context::new_with_codec(codec);
            encoder_ctx.set_threading(Config::count(thread_count));
            let mut encoder = encoder_ctx.encoder().video()?;

            encoder.set_width(output_width);
            encoder.set_height(output_height);
            encoder.set_format(ffmpeg::format::Pixel::VIDEOTOOLBOX);
            encoder.set_time_base(input_config.time_base);
            encoder.set_frame_rate(Some(input_config.frame_rate));
            self.configure(&mut encoder);

            unsafe { hw_frames.attach_to_encoder(encoder.as_mut_ptr()) };

            encoder.open_as_with(codec, self.options())?
        };

        info!(
            encoder = %codec.name(),
            width = output_width,
            height = output_height,
            fps = input_config.frame_rate.0 as f32 / input_config.frame_rate.1.max(1) as f32,
            "Selected VideoToolbox encoder with zero-copy input"
        );

        Ok(H264Encoder {
            base: EncoderBase::new(0),
            codec,
            kind: self.codec,
            frame_rate: input_config.frame_rate,
            encoder,
            converter: None,
            output_format: ffmpeg::format::Pixel::VIDEOTOOLBOX,
            output_width,
            output_height,
            input_format: input_config.pixel_format,
            input_width: input_config.width,
            input_height: input_config.height,
            converted_frame_pool: None,
            videotoolbox_hw: Some(Arc::new(hw_frames)),
        })
    }
}

/// Klipolo: the output stream at a 90 kHz time base; HEVC is tagged `hvc1` (QuickTime and Apple devices play it).
fn add_stream(
    output: &mut format::context::Output,
    codec: Codec,
    kind: VtCodec,
    frame_rate: ffmpeg::Rational,
    params: impl Into<ffmpeg::codec::Parameters>,
) -> Result<usize, H264EncoderError> {
    let mut output_stream = output.add_stream(codec)?;
    let stream_index = output_stream.index();
    output_stream.set_time_base((1, H264Encoder::TIME_BASE));
    output_stream.set_rate(frame_rate);
    output_stream.set_parameters(params);
    if kind == VtCodec::HevcMain10 {
        unsafe {
            let tag = u32::from_le_bytes(*b"hvc1");
            (*(*output_stream.as_mut_ptr()).codecpar).codec_tag = tag;
        }
    }
    Ok(stream_index)
}

/// Klipolo: what adding an encoder's stream to an output takes (`H264Encoder::stream_template`).
pub struct StreamTemplate {
    codec: Codec,
    kind: VtCodec,
    frame_rate: ffmpeg::Rational,
    params: ffmpeg::codec::Parameters,
}

impl StreamTemplate {
    /// Adds the stream to `output`; returns its index.
    pub fn add_to(&self, output: &mut format::context::Output) -> Result<usize, H264EncoderError> {
        add_stream(
            output,
            self.codec,
            self.kind,
            self.frame_rate,
            self.params.clone(),
        )
    }
}

pub(crate) struct OpenedVideoEncoder {
    pub encoder: encoder::Video,
    pub converter: Option<ffmpeg::software::scaling::Context>,
    pub converted_frame_pool: Option<frame::Video>,
    pub output_format: format::Pixel,
    pub output_width: u32,
    pub output_height: u32,
    pub input_format: format::Pixel,
    pub input_width: u32,
    pub input_height: u32,
}

fn is_rgb(format: format::Pixel) -> bool {
    format.descriptor().is_some_and(|descriptor| unsafe {
        (*descriptor.as_ptr()).flags & ffmpeg::ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0
    })
}

fn configure_rgb_converter(
    converter: &mut ffmpeg::software::scaling::Context,
    input: format::Pixel,
    output: format::Pixel,
) -> Result<(), H264EncoderError> {
    if !is_rgb(input) || is_rgb(output) {
        return Ok(());
    }

    // swscale defaults to BT.601 even though the encoder declares BT.709.
    // RGB input is full range; the encoded YUV contract below is limited range.
    let result = unsafe {
        let coefficients = ffmpeg::ffi::sws_getCoefficients(ffmpeg::ffi::SWS_CS_ITU709);
        ffmpeg::ffi::sws_setColorspaceDetails(
            converter.as_mut_ptr(),
            coefficients,
            1,
            coefficients,
            0,
            0,
            1 << 16,
            1 << 16,
        )
    };
    if result < 0 {
        return Err(H264EncoderError::ColorConversion(ffmpeg::Error::from(
            result,
        )));
    }
    Ok(())
}

fn open_video_encoder_inner(
    builder: &H264EncoderBuilder,
    codec: Codec,
    encoder_options: Dictionary<'static>,
    input_config: &VideoInfo,
) -> Result<OpenedVideoEncoder, H264EncoderError> {
    let output_width = input_config.width;
    let output_height = input_config.height;
    let supported: Vec<format::Pixel> = codec
        .video()
        .ok()
        .and_then(|codec_video| codec_video.formats())
        .map(|formats| formats.collect())
        .unwrap_or_default();

    let output_format = if supported.contains(&input_config.pixel_format) {
        input_config.pixel_format
    } else {
        // Klipolo: the first software format the encoder lists (VIDEOTOOLBOX is the hardware-frame format).
        supported
            .iter()
            .copied()
            .find(|f| *f != format::Pixel::VIDEOTOOLBOX)
            .unwrap_or(format::Pixel::NV12)
    };
    let needs_pixel_conversion = output_format != input_config.pixel_format;

    debug!(
        encoder = %codec.name(),
        input_format = ?input_config.pixel_format,
        output_format = ?output_format,
        needs_pixel_conversion,
        "Encoder pixel format configuration"
    );

    if is_420(output_format)
        && (!output_width.is_multiple_of(2) || !output_height.is_multiple_of(2))
    {
        return Err(H264EncoderError::InvalidOutputDimensions {
            width: output_width,
            height: output_height,
        });
    }

    let converter = if needs_pixel_conversion {
        match ffmpeg::software::scaling::Context::get(
            input_config.pixel_format,
            input_config.width,
            input_config.height,
            output_format,
            output_width,
            output_height,
            ffmpeg::software::scaling::flag::Flags::FAST_BILINEAR,
        ) {
            Ok(mut context) => {
                configure_rgb_converter(&mut context, input_config.pixel_format, output_format)?;
                debug!(
                    encoder = %codec.name(),
                    src_format = ?input_config.pixel_format,
                    dst_format = ?output_format,
                    "Created SOFTWARE scaler for pixel format conversion (CPU-intensive)"
                );
                Some(context)
            }
            Err(e) => {
                error!(
                    "Failed to create converter from {:?} to {:?}: {:?}",
                    input_config.pixel_format, output_format, e
                );
                return Err(H264EncoderError::PixFmtNotSupported(
                    input_config.pixel_format,
                ));
            }
        }
    } else {
        None
    };

    let thread_count = thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(1);

    let encoder = {
        let mut encoder_ctx = context::Context::new_with_codec(codec);
        encoder_ctx.set_threading(Config::count(thread_count));
        let mut encoder = encoder_ctx.encoder().video()?;

        encoder.set_width(output_width);
        encoder.set_height(output_height);
        encoder.set_format(output_format);
        encoder.set_time_base(input_config.time_base);
        encoder.set_frame_rate(Some(input_config.frame_rate));
        builder.configure(&mut encoder);

        encoder.open_as_with(codec, encoder_options)?
    };

    let converted_frame_pool = converter
        .as_ref()
        .map(|_| frame::Video::new(output_format, output_width, output_height));

    Ok(OpenedVideoEncoder {
        encoder,
        converter,
        converted_frame_pool,
        output_format,
        output_width,
        output_height,
        input_format: input_config.pixel_format,
        input_width: input_config.width,
        input_height: input_config.height,
    })
}

pub struct H264Encoder {
    base: EncoderBase,
    codec: Codec,
    kind: VtCodec,
    frame_rate: ffmpeg::Rational,
    encoder: encoder::Video,
    converter: Option<ffmpeg::software::scaling::Context>,
    output_format: format::Pixel,
    output_width: u32,
    output_height: u32,
    input_format: format::Pixel,
    input_width: u32,
    input_height: u32,
    converted_frame_pool: Option<frame::Video>,
    #[cfg(target_os = "macos")]
    videotoolbox_hw: Option<Arc<VideoToolboxHwFrames>>,
}

pub struct ConversionRequirements {
    pub input_format: format::Pixel,
    pub input_width: u32,
    pub input_height: u32,
    pub output_format: format::Pixel,
    pub output_width: u32,
    pub output_height: u32,
    pub needs_conversion: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum QueueFrameError {
    #[error("Converter: {0}")]
    Converter(ffmpeg::Error),
    #[error("Encode: {0}")]
    Encode(ffmpeg::Error),
}

impl H264Encoder {
    const TIME_BASE: i32 = 90000;

    pub fn builder(input_config: VideoInfo) -> H264EncoderBuilder {
        H264EncoderBuilder::new(input_config)
    }

    /// Klipolo: this encoder's stream (codec parameters, 90 kHz time base, `hvc1` tag), detached from it: a packet
    /// sink adds it to each new segment file while the encoder runs.
    pub fn stream_template(&self) -> StreamTemplate {
        StreamTemplate {
            codec: self.codec,
            kind: self.kind,
            frame_rate: self.frame_rate,
            params: (&self.encoder).into(),
        }
    }

    fn attach(&mut self, output: &mut format::context::Output) -> Result<(), H264EncoderError> {
        let stream_index = add_stream(
            output,
            self.codec,
            self.kind,
            self.frame_rate,
            &self.encoder,
        )?;
        self.base = EncoderBase::new(stream_index);
        Ok(())
    }

    /// Klipolo: the codec's parameter sets (`avcC`/`hvcC` payload; empty for ProRes).
    pub fn extradata(&self) -> &[u8] {
        unsafe {
            let ctx = self.encoder.as_ptr();
            if (*ctx).extradata.is_null() || (*ctx).extradata_size <= 0 {
                return &[];
            }
            std::slice::from_raw_parts((*ctx).extradata, (*ctx).extradata_size as usize)
        }
    }

    /// Klipolo: encodes `frame` as `pts` (in the encoder's time base, 1/fps), as a keyframe when `key`, and hands every
    /// packet that is ready to `sink` (encoder time base).
    pub fn encode_to(
        &mut self,
        frame: &mut frame::Video,
        pts: i64,
        key: bool,
        sink: &mut dyn FnMut(&mut Packet) -> Result<(), ffmpeg::Error>,
    ) -> Result<(), QueueFrameError> {
        let kind = if key {
            ffmpeg::picture::Type::I
        } else {
            ffmpeg::picture::Type::None
        };
        frame.set_pts(Some(pts));
        frame.set_kind(kind);
        let frame_to_send = if let Some(converter) = &mut self.converter {
            let converted = self.converted_frame_pool.as_mut().unwrap();
            converter
                .run(frame, converted)
                .map_err(QueueFrameError::Converter)?;
            converted.set_pts(Some(pts));
            converted.set_kind(kind);
            converted as &frame::Video
        } else {
            frame as &frame::Video
        };
        self.base
            .send_frame_to(frame_to_send, &mut self.encoder, sink)
            .map_err(QueueFrameError::Encode)
    }

    /// Klipolo: ends the stream and hands the remaining packets to `sink`.
    pub fn flush_to(
        &mut self,
        sink: &mut dyn FnMut(&mut Packet) -> Result<(), ffmpeg::Error>,
    ) -> Result<(), ffmpeg::Error> {
        self.base.process_eof_to(&mut self.encoder, sink)
    }

    /// Klipolo: the encoder's time base (1/fps for export).
    pub fn time_base(&self) -> ffmpeg::Rational {
        self.encoder.time_base()
    }

    /// Whether this encoder was built with
    /// [`H264EncoderBuilder::build_videotoolbox_hw_input`] and expects
    /// wrapped CVPixelBuffer frames instead of software frames.
    #[cfg(target_os = "macos")]
    pub fn is_videotoolbox_hw_input(&self) -> bool {
        self.videotoolbox_hw.is_some()
    }

    /// Wraps an IOSurface-backed `CVPixelBufferRef` as a frame this
    /// encoder can queue directly. Only available on encoders built with
    /// [`H264EncoderBuilder::build_videotoolbox_hw_input`].
    ///
    /// # Safety
    /// `pixel_buffer` must be a valid `CVPixelBufferRef` matching the
    /// encoder's dimensions and input pixel format.
    #[cfg(target_os = "macos")]
    pub unsafe fn wrap_videotoolbox_pixel_buffer(
        &self,
        pixel_buffer: *mut std::ffi::c_void,
    ) -> Result<frame::Video, H264EncoderError> {
        unsafe { self.wrap_videotoolbox_pixel_buffer_then(pixel_buffer, None) }
    }

    /// Klipolo: `wrap_videotoolbox_pixel_buffer`, calling `released` once VideoToolbox lets the buffer go (it output
    /// the frame, or dropped it), also when wrapping fails.
    ///
    /// # Safety
    /// As `wrap_videotoolbox_pixel_buffer`.
    #[cfg(target_os = "macos")]
    pub unsafe fn wrap_videotoolbox_pixel_buffer_then(
        &self,
        pixel_buffer: *mut std::ffi::c_void,
        released: Option<super::videotoolbox_hw::Released>,
    ) -> Result<frame::Video, H264EncoderError> {
        let Some(hw_frames) = self.videotoolbox_hw.as_ref() else {
            if let Some(released) = released {
                released();
            }
            return Err(H264EncoderError::VideoToolboxHwInput(
                "encoder was not built for VideoToolbox hardware input".to_string(),
            ));
        };
        unsafe { hw_frames.wrap_pixel_buffer_then(pixel_buffer, released) }
            .map_err(|e| H264EncoderError::VideoToolboxHwInput(e.to_string()))
    }

    pub fn conversion_requirements(&self) -> ConversionRequirements {
        let needs_conversion = self.input_format != self.output_format
            || self.input_width != self.output_width
            || self.input_height != self.output_height;
        ConversionRequirements {
            input_format: self.input_format,
            input_width: self.input_width,
            input_height: self.input_height,
            output_format: self.output_format,
            output_width: self.output_width,
            output_height: self.output_height,
            needs_conversion,
        }
    }

    pub fn queue_frame(
        &mut self,
        frame: &mut frame::Video,
        timestamp: Duration,
        output: &mut format::context::Output,
    ) -> Result<(), QueueFrameError> {
        self.base.update_pts(frame, timestamp, &mut self.encoder);

        let frame_to_send = if let Some(converter) = &mut self.converter {
            let pts = frame.pts();
            let converted = self.converted_frame_pool.as_mut().unwrap();
            converter
                .run(frame, converted)
                .map_err(QueueFrameError::Converter)?;
            converted.set_pts(pts);
            converted as &frame::Video
        } else {
            frame as &frame::Video
        };

        trace!(pts = ?frame_to_send.pts(), "queue video frame");
        self.base
            .send_frame(frame_to_send, output, &mut self.encoder)
            .map_err(QueueFrameError::Encode)?;

        Ok(())
    }

    pub fn flush(&mut self, output: &mut format::context::Output) -> Result<(), ffmpeg::Error> {
        self.base.process_eof(output, &mut self.encoder)
    }

    pub fn set_packet_stats(&mut self, stats: Arc<super::super::base::EncodedPacketStats>) {
        self.base.set_packet_stats(stats);
    }
}

// Klipolo: moved here with `mux/mp4.rs`'s removal (the encoder is owned by one thread at a time).
unsafe impl Send for H264Encoder {}
