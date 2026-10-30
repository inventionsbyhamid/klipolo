//! Which sources get an editing copy and how big. Pure: the facts come from `facts`, the
//! policy from the GPU class and this computer's encoders.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::facts::SourceFacts;
use crate::export::shared::FrameRate;
use crate::platform::encode::{CodecRole, candidates};
use crate::platform::gpu::GpuClass;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProxyPolicy {
    /// Sources with more pixels than this get a copy.
    pub max_pixels: u64,
    /// More-than-8-bit sources with more pixels than this get a copy, even when the hardware path reads them: on an
    /// Intel Mac each 4K Main 10 HEVC reader holds about 190 MB (VideoToolbox's service and ours, measured with
    /// `perf_harness vt-memory`: six readers +1.3 GB) against about 60 MB for 8-bit 4K H.264 (six +0.4 GB), past the
    /// app's 1.5 GB budget. Apple silicon's media engine isn't measured yet: its limit stays 4K.
    pub max_deep_pixels: u64,
    /// The copy's long side.
    pub long_side: u32,
    /// Whether this computer can make a copy at all (an H.264 encoder through `platform::encode`). False on Windows
    /// (W) and Linux (L) until their encoders land: then nothing is queued that can only fail.
    pub can_encode: bool,
}

impl ProxyPolicy {
    /// The policy for this computer: its GPU class (from the engine, `EngineInfo.gpu_class`) and its encoders.
    pub fn for_gpu(class: GpuClass) -> Self {
        Self::with_encoder(class, !candidates(CodecRole::H264).is_empty())
    }

    /// The policy for `class` when an H.264 encoder does (or doesn't) exist; pure, for tests.
    pub fn with_encoder(class: GpuClass, can_encode: bool) -> Self {
        let (max_pixels, long_side) = match class {
            GpuClass::Integrated | GpuClass::Software => (1920 * 1080, 1280),
            GpuClass::Apple | GpuClass::Discrete => (3840 * 2160, 1920),
        };
        let max_deep_pixels = match class {
            GpuClass::Apple => 3840 * 2160,
            GpuClass::Integrated | GpuClass::Software | GpuClass::Discrete => 1920 * 1080,
        };
        Self {
            max_pixels,
            long_side,
            max_deep_pixels,
            can_encode,
        }
    }
}

/// Why a source gets an editing copy: it scrubs slowly as it is. `TenBit`: more than 8 bits that this computer
/// decodes in software (H.264 High 10, 4:2:2, ProRes), or that is larger than `ProxyPolicy::max_deep_pixels` (its
/// readers cost too much memory). VideoToolbox reads 10-bit 4:2:0 HEVC on the hardware path,
/// so a phone's 1080p Main 10 clip gets no copy for its depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProxyReason {
    NoHardwareDecode,
    TenBit,
    Hdr,
    TooLarge,
    LongGop,
    VariableRate,
}

/// Why `f` scrubs slowly enough to need an editing copy, or None. Always None when no copy can be made here.
pub fn needs_proxy(f: &SourceFacts, policy: &ProxyPolicy) -> Option<ProxyReason> {
    if !policy.can_encode {
        None
    } else if !f.hw_decodes {
        Some(ProxyReason::NoHardwareDecode)
    } else if f.ten_bit
        && (!f.ten_bit_hw || u64::from(f.width) * u64::from(f.height) > policy.max_deep_pixels)
    {
        Some(ProxyReason::TenBit)
    } else if f.hdr {
        Some(ProxyReason::Hdr)
    } else if u64::from(f.width) * u64::from(f.height) > policy.max_pixels {
        Some(ProxyReason::TooLarge)
    } else if f.max_key_interval_us > 1_000_000 {
        Some(ProxyReason::LongGop)
    } else if f.variable_rate {
        Some(ProxyReason::VariableRate)
    } else {
        None
    }
}

/// The copy's size: long side `policy.long_side` (never upscaled), the source's shape, even dimensions.
pub fn proxy_size(f: &SourceFacts, policy: &ProxyPolicy) -> (u32, u32) {
    let long = f.width.max(f.height);
    let scale = (f64::from(policy.long_side.min(long)) / f64::from(long.max(1))).min(1.0);
    let even = |v: u32| ((f64::from(v) * scale / 2.0).round() as u32 * 2).max(2);
    (even(f.width), even(f.height))
}

/// About 0.1 bit per pixel (720p30 ≈ 2.8 Mb/s), at least 1.5 Mb/s.
pub fn proxy_bitrate(w: u32, h: u32, fps: f64) -> u32 {
    ((f64::from(w) * f64::from(h) * fps * 0.1) as u32).max(1_500_000)
}

/// The copy's frame rate: the source's own, exactly (24000/1001 stays 24000/1001), so copy frame n is the source frame
/// the preview shows as frame n, and both run on one grid (a rounded rate showed the frame before at
/// most stills). None when the file states no usable rate (no copy is made: `job::Refused::UnusualRate`).
pub fn proxy_rate(f: &SourceFacts) -> Option<FrameRate> {
    let fps = f.fps();
    (f.rate.num > 0 && f.rate.den > 0 && (1.0..=MAX_COPY_FPS).contains(&fps)).then_some(f.rate)
}

/// Rates above this aren't copied (a damaged header; a 240 fps phone clip is the fastest kept).
pub const MAX_COPY_FPS: f64 = 240.0;
