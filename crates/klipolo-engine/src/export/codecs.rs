//! Which encoder and pixel layout each export codec uses (VideoToolbox on a Mac; `platform::encode` lists each OS's
//! encoders), and the probe that finds the encoders this computer has.

use std::time::Duration;

use ffmpeg::format::Pixel;

use super::quality::VideoCodec;
use crate::platform::encode::{self, CodecRole, EncoderChoice};
use crate::vendor::cap::enc_ffmpeg::h264::{H264Encoder, H264EncoderBuilder, VtCodec};
use crate::vendor::cap::media_info::VideoInfo;

/// The vendored encoder's settings for `codec` (its profile and options).
fn vt_codec(codec: VideoCodec) -> VtCodec {
    match codec {
        VideoCodec::H264High => VtCodec::H264High,
        VideoCodec::HevcMain10 => VtCodec::HevcMain10,
        VideoCodec::ProRes422Hq => VtCodec::ProRes422Hq,
    }
}

fn role(codec: VideoCodec) -> CodecRole {
    match codec {
        VideoCodec::H264High => CodecRole::H264,
        VideoCodec::HevcMain10 => CodecRole::Hevc10,
        VideoCodec::ProRes422Hq => CodecRole::ProRes,
    }
}

/// The encoders `codec` may use on this computer, best first.
fn candidates(codec: VideoCodec) -> &'static [EncoderChoice] {
    encode::candidates(role(codec))
}

/// The first candidate this FFmpeg build has (None: this computer has no encoder for `codec`). Opens nothing.
pub fn encoder_name(codec: VideoCodec) -> Option<&'static str> {
    candidates(codec)
        .iter()
        .map(|c| c.ffmpeg_name)
        .find(|name| ffmpeg::encoder::find_by_name(name).is_some())
}

/// The builder for `codec` on its first available encoder (None: there is none).
pub(crate) fn builder(info: VideoInfo, codec: VideoCodec) -> Option<H264EncoderBuilder> {
    Some(
        H264Encoder::builder(info)
            .with_codec(vt_codec(codec))
            .with_encoder_name(encoder_name(codec)?),
    )
}

/// ProRes's 4:2:2 10-bit input: always P210, the one 4:2:2 layout the GPU writes (`Yuv::from_pixel` knows NV12,
/// P010 and P210 only). It is the first 4:2:2 10-bit format prores_videotoolbox lists, so a Mac is unchanged. prores_ks
/// (Linux, Windows) lists planar yuv422p10le first: asking the encoder for its first 4:2:2 format made `pass.rs` fall
/// back to P010 while the encoder read the frame as planar 4:2:2, and every picture came out as garbage. The encoder
/// converts P210 to its own format with swscale.
fn prores_input() -> Pixel {
    Pixel::P210LE
}

/// The pixel format the GPU makes for `codec`: NV12 for H.264, P010 for HEVC Main 10, 10-bit 4:2:2 for ProRes.
pub fn input_format(codec: VideoCodec) -> Pixel {
    match codec {
        VideoCodec::H264High => Pixel::NV12,
        VideoCodec::HevcMain10 => Pixel::P010LE,
        VideoCodec::ProRes422Hq => prores_input(),
    }
}

/// Whether `codec` encodes here: each candidate (`hardware_only`: only hardware ones, refusing their software
/// fallback) opens a tiny one-frame session and encodes a frame, until one works.
pub fn probe(codec: VideoCodec, hardware_only: bool) -> bool {
    candidates(codec)
        .iter()
        .filter(|c| c.hardware || !hardware_only)
        .any(|c| probe_one(codec, c.ffmpeg_name, hardware_only))
}

fn probe_one(codec: VideoCodec, name: &'static str, hardware_only: bool) -> bool {
    let run = || -> Result<(), String> {
        ffmpeg::init().map_err(|e| e.to_string())?;
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let path = dir.path().join("probe.mov");
        let mut output = ffmpeg::format::output_as(&path, "mov").map_err(|e| e.to_string())?;
        let format = input_format(codec);
        let (w, h) = (PROBE_SIDE, PROBE_SIDE);
        let mut encoder = H264Encoder::builder(VideoInfo::from_raw_ffmpeg(format, w, h, 30))
            .with_codec(vt_codec(codec))
            .with_encoder_name(name)
            .with_bitrate(if codec == VideoCodec::ProRes422Hq {
                0
            } else {
                1_000_000
            })
            .with_hardware_only(hardware_only)
            .build(&mut output)
            .map_err(|e| e.to_string())?;
        output.write_header().map_err(|e| e.to_string())?;
        let mut frame = ffmpeg::frame::Video::new(format, w, h);
        for plane in 0..frame.planes() {
            frame.data_mut(plane).fill(0x80);
        }
        encoder
            .queue_frame(&mut frame, Duration::ZERO, &mut output)
            .map_err(|e| e.to_string())?;
        encoder.flush(&mut output).map_err(|e| e.to_string())?;
        output.write_trailer().map_err(|e| e.to_string())
    };
    match run() {
        Ok(()) => true,
        Err(error) => {
            tracing::info!(?codec, name, hardware_only, %error, "encoder probe failed");
            false
        }
    }
}

/// Probe sessions are this many pixels square (VideoToolbox's hardware H.264 refuses 64 × 64 and HEVC Main 10 encodes
/// nothing below 192 × 192 on Intel).
const PROBE_SIDE: u32 = 256;
