//! The hardware decode seam: which FFmpeg hardware device the FFmpeg decoder opens, and which codecs this
//! computer decodes in hardware (editing copies).

use ffmpeg::sys::AVHWDeviceType;

/// macOS: VideoToolbox. Windows (W: `D3D12VA`, on wgpu's own D3D12 device) and Linux (L: `VAAPI`, exported as
/// DMA-BUF) decode in software for now.
pub fn hw_device_type() -> Option<AVHWDeviceType> {
    if cfg!(target_os = "macos") {
        Some(AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX)
    } else {
        None
    }
}

/// Whether this computer decodes `codec` in hardware (no editing copy needed for that reason).
/// macOS: VideoToolbox decodes H.264 and HEVC everywhere and ProRes on Apple silicon with media engines (treated as
/// software on Intel). VideoToolbox itself is asked (`VTIsHardwareDecodeSupported`), so an Intel Mac
/// without HEVC hardware, ProRes without a ProRes engine, VP9 or AV1 without a decoder for them, and a VM without a
/// media engine all answer false. Windows (W): D3D12/DXVA by probe. Linux (L): VA-API by probe. Stubs: false.
pub fn hw_decodes(codec: ffmpeg::codec::Id) -> bool {
    #[cfg(target_os = "macos")]
    {
        use cidre::{cm::VideoCodec, vt};
        use ffmpeg::codec::Id;
        let fourcc = match codec {
            Id::H264 => VideoCodec::H264,
            Id::HEVC => VideoCodec::HEVC,
            Id::PRORES => VideoCodec::APPLE_PRO_RES422,
            Id::VP9 => VideoCodec::VP9,
            Id::AV1 => VideoCodec::AV1,
            Id::MJPEG => VideoCodec::JPEG,
            _ => return false,
        };
        vt::is_hardware_decode_supported(fourcc)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = codec;
        false
    }
}

/// Whether this computer's preview decoder reads `codec` frames stored as `pixel` (more than 8 bits) on the hardware
/// path, so a 10-bit source scrubs like an 8-bit one. macOS: VideoToolbox through the vendored AVAssetReader takes
/// 10-bit 4:2:0 HEVC (phones' Main 10; its table is `avassetreader::output_pixel_format`);
/// H.264 High 10, 4:2:2 and ProRes go through FFmpeg's software decoder. W/L: false (their decoders are software).
pub fn hw_reads_deep(codec: ffmpeg::codec::Id, pixel: ffmpeg::format::Pixel) -> bool {
    #[cfg(target_os = "macos")]
    {
        use ffmpeg::format::Pixel;
        codec == ffmpeg::codec::Id::HEVC
            && matches!(pixel, Pixel::YUV420P10LE | Pixel::P010LE)
            && hw_decodes(codec)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (codec, pixel);
        false
    }
}
