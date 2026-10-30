//! The encoder seam: the FFmpeg encoders each codec role may use on this OS, best first. Export opens
//! the first one this FFmpeg build has, and the probe tries each.
//! - macOS: VideoToolbox, exactly as before.
//! - Windows and Linux: `prores_ks` (software) for ProRes, and nothing for H.264 or HEVC yet: Klipolo's LGPL FFmpeg has
//!   no software H.264 encoder, so an export there says this computer has no H.264 encoder. W adds NVENC, AMF, QSV,
//!   D3D12 and Media Foundation; L adds VA-API, NVENC and Vulkan.
//! - Every OS: FFmpeg's own `mjpeg` (LGPL, software) for stills (thumbnail sprites).

/// What an encoder is for (the codecs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecRole {
    /// H.264 High, 8-bit 4:2:0.
    H264,
    /// HEVC Main 10.
    Hevc10,
    /// ProRes 422 HQ.
    ProRes,
    /// A picture: thumbnail sprites.
    Still,
}

/// Who does the encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    VideoToolbox,
    Nvenc,
    Amf,
    Qsv,
    D3d12,
    MediaFoundation,
    Vaapi,
    Vulkan,
    Software,
}

/// One FFmpeg encoder a role can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderChoice {
    pub ffmpeg_name: &'static str,
    pub backend: Backend,
    /// Runs on a media engine rather than the CPU (VideoToolbox may still fall back to software for ProRes on Intel).
    pub hardware: bool,
}

const fn choice(ffmpeg_name: &'static str, backend: Backend, hardware: bool) -> EncoderChoice {
    EncoderChoice {
        ffmpeg_name,
        backend,
        hardware,
    }
}

/// Extra options for work nobody is waiting for (editing copies): VideoToolbox's `power_efficient=1` (Apple's offline
/// transcoding guidance). Windows (W) and Linux (L) add their low-power hints with their encoders. None elsewhere.
pub fn background_options(choice: &EncoderChoice) -> &'static [(&'static str, &'static str)] {
    match choice.backend {
        Backend::VideoToolbox => &[("power_efficient", "1")],
        _ => &[],
    }
}

/// The encoders `role` may use on this OS, best first (empty: none).
pub fn candidates(role: CodecRole) -> &'static [EncoderChoice] {
    const STILL: [EncoderChoice; 1] = [choice("mjpeg", Backend::Software, false)];
    #[cfg(target_os = "macos")]
    {
        const H264: [EncoderChoice; 1] = [choice("h264_videotoolbox", Backend::VideoToolbox, true)];
        const HEVC: [EncoderChoice; 1] = [choice("hevc_videotoolbox", Backend::VideoToolbox, true)];
        const PRORES: [EncoderChoice; 1] =
            [choice("prores_videotoolbox", Backend::VideoToolbox, true)];
        match role {
            CodecRole::H264 => &H264,
            CodecRole::Hevc10 => &HEVC,
            CodecRole::ProRes => &PRORES,
            CodecRole::Still => &STILL,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        const PRORES: [EncoderChoice; 1] = [choice("prores_ks", Backend::Software, false)];
        match role {
            CodecRole::H264 | CodecRole::Hevc10 => &[],
            CodecRole::ProRes => &PRORES,
            CodecRole::Still => &STILL,
        }
    }
}
