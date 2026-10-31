// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/video/h264.rs
// (the vendor priority, lines 1093-1160, and the live encoder options, lines 1260-1320), copied as data by hand.
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes:
// - `libx264` is dropped from every ladder (GPL; Klipolo's FFmpeg is the LGPL build), along with Cap's "no hardware
//   encoding → libx264" branch: no hardware encoder means no video recording.
// - The HEVC ladders mirror the H.264 ones with `hevc_*`.
// - Options: QSV's look-ahead is off (`look_ahead=0`; Cap's live set has 1); every encoder gets `bf=0` (B-frames off);
//   `g` is the caller's, one second (Cap: 2 s); `mf`'s `hw_encoding` is written `1` (Cap: "true", the same boolean).
// - The Linux ladder and the `vaapi` and `vulkan` options are Klipolo's own: Cap's Linux ladder is `h264_nvenc`, then
//   `libx264`.
// - The vendor comes from the caller (`GpuVendor`), not Cap's `detect_primary_gpu`.

//! The FFmpeg hardware encoders Windows and Linux try, best first, and their low-latency capture options. macOS records
//! through VideoToolbox with a pinned encoder, so it has no ladder here. The ladder is tried in order once, at
//! the start of a run; the encoders after the one that opened are never a
//! mid-take fallback.

use klipolo_platform::caps::OsFamily;

use crate::media::Codec;

/// The GPU an encoder ladder is picked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
}

/// (H.264, HEVC), best first.
type Ladders = (&'static [&'static str], &'static [&'static str]);

// Windows: Cap's `ENCODER_PRIORITY_{NVIDIA,AMD,INTEL,DEFAULT}` without `libx264`; Media Foundation (`mf`) comes second
// for a known vendor and last for any other.
const WINDOWS_NVIDIA: Ladders = (
    &["h264_nvenc", "h264_mf", "h264_qsv", "h264_amf"],
    &["hevc_nvenc", "hevc_mf", "hevc_qsv", "hevc_amf"],
);
const WINDOWS_AMD: Ladders = (
    &["h264_amf", "h264_mf", "h264_nvenc", "h264_qsv"],
    &["hevc_amf", "hevc_mf", "hevc_nvenc", "hevc_qsv"],
);
const WINDOWS_INTEL: Ladders = (
    &["h264_qsv", "h264_mf", "h264_nvenc", "h264_amf"],
    &["hevc_qsv", "hevc_mf", "hevc_nvenc", "hevc_amf"],
);
const WINDOWS_OTHER: Ladders = (
    &["h264_nvenc", "h264_qsv", "h264_amf", "h264_mf"],
    &["hevc_nvenc", "hevc_qsv", "hevc_amf", "hevc_mf"],
);

// Linux (Klipolo's own): VA-API for Intel and AMD, NVENC first on NVIDIA, Vulkan Video last everywhere (the newest);
// an unknown vendor tries VA-API, then NVENC.
const LINUX_VAAPI: Ladders = (
    &["h264_vaapi", "h264_vulkan"],
    &["hevc_vaapi", "hevc_vulkan"],
);
const LINUX_NVIDIA: Ladders = (
    &["h264_nvenc", "h264_vaapi", "h264_vulkan"],
    &["hevc_nvenc", "hevc_vaapi", "hevc_vulkan"],
);
const LINUX_OTHER: Ladders = (
    &["h264_vaapi", "h264_nvenc", "h264_vulkan"],
    &["hevc_vaapi", "hevc_nvenc", "hevc_vulkan"],
);

/// FFmpeg encoders to try for H.264 (or HEVC) capture on Windows and Linux, best first: Windows from Cap's ladders
/// (no libx264); Linux is Klipolo's own (Cap's Linux ladder is nvenc, then libx264). Empty on macOS.
pub fn ladder(os: OsFamily, vendor: GpuVendor, codec: Codec) -> &'static [&'static str] {
    let (h264, hevc) = match (os, vendor) {
        (OsFamily::Macos, _) => return &[],
        (OsFamily::Windows, GpuVendor::Nvidia) => WINDOWS_NVIDIA,
        (OsFamily::Windows, GpuVendor::Amd) => WINDOWS_AMD,
        (OsFamily::Windows, GpuVendor::Intel) => WINDOWS_INTEL,
        (OsFamily::Windows, GpuVendor::Apple | GpuVendor::Other) => WINDOWS_OTHER,
        (OsFamily::Linux, GpuVendor::Nvidia) => LINUX_NVIDIA,
        (OsFamily::Linux, GpuVendor::Amd | GpuVendor::Intel) => LINUX_VAAPI,
        (OsFamily::Linux, GpuVendor::Apple | GpuVendor::Other) => LINUX_OTHER,
    };
    match codec {
        Codec::H264 => h264,
        Codec::Hevc => hevc,
    }
}

/// Cap's low-latency capture options per FFmpeg encoder (B-frames off, `g` = one second added by the caller), for the
/// H.264 and HEVC encoder of each API alike. An option an encoder build doesn't know (QSV's `look_ahead` under oneVPL)
/// is left over unused by `avcodec_open2`, not an error. Vulkan Video, and any encoder not listed, gets `bf=0` only.
pub fn live_options(encoder: &str) -> &'static [(&'static str, &'static str)] {
    let api = encoder
        .strip_prefix("h264_")
        .or_else(|| encoder.strip_prefix("hevc_"))
        .unwrap_or(encoder);
    match api {
        "nvenc" => &[
            ("preset", "p4"),
            ("tune", "ll"),
            ("rc", "vbr"),
            ("spatial-aq", "1"),
            ("temporal-aq", "1"),
            ("bf", "0"),
        ],
        "qsv" => &[("preset", "faster"), ("look_ahead", "0"), ("bf", "0")],
        "amf" => &[("quality", "balanced"), ("rc", "vbr_latency"), ("bf", "0")],
        "mf" => &[
            ("hw_encoding", "1"),
            ("scenario", "4"),
            ("quality", "1"),
            ("bf", "0"),
        ],
        "vaapi" => &[("rc_mode", "VBR"), ("bf", "0")],
        _ => &[("bf", "0")],
    }
}
