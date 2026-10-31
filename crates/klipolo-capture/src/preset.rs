//! The quality presets: each one's size, frame rate and
//! bitrate per codec, and what a take at them writes per second (the disk outlook and preflight's "About {minutes} min
//! of space"). Native sizes follow the display (`native_preset`); the rest are fixed.

use crate::backend::PresetId;
use crate::media::Codec;

/// 24-bit PCM at 48 kHz, one channel: the mic.
pub const PCM_MONO_BPS: u64 = 1_152_000;
/// 24-bit PCM at 48 kHz, two channels: game or computer sound.
pub const PCM_STEREO_BPS: u64 = 2_304_000;
/// The longest side a native recording keeps (4K); larger displays are scaled down to it.
pub const NATIVE_MAX_SIDE: u32 = 3840;
/// A native recording's bitrate never goes above this.
pub const NATIVE_MAX_BPS: u64 = 80_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: PresetId,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u64,
}

const MBPS: u64 = 1_000_000;

/// The fixed presets. `Native30` and `Native60` need the display's size (`native_preset`); without one they are taken
/// at 1920×1080.
pub fn preset(id: PresetId, codec: Codec) -> Preset {
    // (width, height, fps, H.264 Mb/s, HEVC Mb/s); the camera presets are H.264 only (`probe::CameraChoice`).
    let (width, height, fps, h264, hevc) = match id {
        PresetId::Light => (1920, 1080, 30, 18, 12),
        PresetId::Smooth => (1920, 1080, 60, 30, 20),
        PresetId::Sharp => (2560, 1440, 60, 50, 32),
        PresetId::SlowMo => (1920, 1080, 120, 45, 30),
        PresetId::Camera720 => (1280, 720, 30, 6, 6),
        PresetId::Camera1080 => (1920, 1080, 30, 10, 10),
        PresetId::Native30 | PresetId::Native60 => return native_preset(id, codec, (1920, 1080)),
    };
    let mbps = match codec {
        Codec::H264 => h264,
        Codec::Hevc => hevc,
    };
    Preset {
        id,
        width,
        height,
        fps,
        bitrate_bps: mbps * MBPS,
    }
}

/// The display's own pixels (the native Retina size), the long side capped at 3840 with even sides; the
/// bitrate is Smooth's scaled by pixels and frame rate, at most 80 Mb/s. `id` other than `Native30`/`Native60` gives
/// that fixed preset, whatever the display.
pub fn native_preset(id: PresetId, codec: Codec, display: (u32, u32)) -> Preset {
    let fps = match id {
        PresetId::Native30 => 30,
        PresetId::Native60 => 60,
        _ => return preset(id, codec),
    };
    let (width, height) = native_size(display);
    let smooth = preset(PresetId::Smooth, codec);
    let reference = u128::from(smooth.width * smooth.height * smooth.fps);
    let scaled =
        u128::from(smooth.bitrate_bps) * u128::from(width) * u128::from(height) * u128::from(fps)
            / reference;
    Preset {
        id,
        width,
        height,
        fps,
        bitrate_bps: scaled.min(u128::from(NATIVE_MAX_BPS)) as u64,
    }
}

/// `display` with its long side at most 3840 (aspect kept) and both sides even (encoders need 4:2:0's whole chroma
/// pairs), at least 2×2.
pub fn native_size((w, h): (u32, u32)) -> (u32, u32) {
    let long = w.max(h);
    let (w, h) = if long > NATIVE_MAX_SIDE {
        let scale =
            |side: u32| (u64::from(side) * u64::from(NATIVE_MAX_SIDE) / u64::from(long)) as u32;
        (scale(w), scale(h))
    } else {
        (w, h)
    };
    ((w & !1).max(2), (h & !1).max(2))
}

/// Our GPU frame pool's budget: a native recording stays inside the recorder's 250 MB footprint, and
/// a slow encoder drops frames (counted) rather than growing memory.
pub const POOL_BYTES: u64 = 64 << 20;

/// How many NV12 frames the capture pool holds at a size and rate: half a second's, capped at `POOL_BYTES`, and at
/// least 2 (one being encoded while the next arrives). 21 at 1080p60 (0.35 s; 30 would take 93 MiB), 10 at
/// 2560×1600, 5 at 4K.
pub fn pool_frames(width: u32, height: u32, fps: u32) -> usize {
    let frame = (u64::from(width) * u64::from(height) * 3 / 2).max(1);
    let fits = POOL_BYTES / frame;
    u64::from(fps / 2).min(fits).max(2) as usize
}

/// The next lighter preset, for an encoder that keeps failing at `id` (the heal policy's `LowerPreset`): the frame
/// rate halves before the size shrinks, and the camera steps to 720p. None at the lightest (Light, Camera720), where a
/// failing encoder can only be rebuilt as it is.
pub fn lighter(id: PresetId) -> Option<PresetId> {
    match id {
        PresetId::Native60 => Some(PresetId::Native30),
        PresetId::Native30 | PresetId::Smooth => Some(PresetId::Light),
        PresetId::Sharp | PresetId::SlowMo => Some(PresetId::Smooth),
        PresetId::Camera1080 => Some(PresetId::Camera720),
        PresetId::Light | PresetId::Camera720 => None,
    }
}

/// What a take writes per second: the video sources' bitrates plus `mono` mic and `stereo` sound tracks of PCM.
pub fn bytes_per_s(screen: Option<Preset>, camera: Option<Preset>, mono: u32, stereo: u32) -> f64 {
    let video: u64 = [screen, camera]
        .iter()
        .flatten()
        .map(|p| p.bitrate_bps)
        .sum();
    let pcm = u64::from(mono) * PCM_MONO_BPS + u64::from(stereo) * PCM_STEREO_BPS;
    (video + pcm) as f64 / 8.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Retina sizes stay as they are; a 5K or 6K display becomes 4K; the bitrate follows the pixels up to 80 Mb/s.
    #[test]
    fn native_sizes_and_bitrates() {
        let mac = native_preset(PresetId::Native60, Codec::Hevc, (3024, 1964));
        assert_eq!((mac.width, mac.height, mac.fps), (3024, 1964, 60));
        assert_eq!(
            mac.bitrate_bps, 57_283_333,
            "Smooth HEVC's 20 Mb/s × 2.86 the pixels"
        );
        let xdr = native_preset(PresetId::Native30, Codec::H264, (6016, 3384));
        assert_eq!((xdr.width, xdr.height, xdr.fps), (3840, 2160, 30));
        assert_eq!(
            native_preset(PresetId::Native60, Codec::H264, (5120, 2880)).bitrate_bps,
            NATIVE_MAX_BPS
        );
        assert_eq!(native_size((1365, 767)), (1364, 766));
        assert_eq!(
            preset(PresetId::Native60, Codec::H264).bitrate_bps,
            30 * MBPS,
            "1080p60 is Smooth's"
        );
    }

    /// About 0.5 s of frames, at most 64 MiB of them.
    #[test]
    fn the_frame_pool_holds_half_a_second_within_64_mib() {
        assert_eq!(
            pool_frames(1920, 1080, 60),
            21,
            "0.35 s: 30 frames would be 93 MiB"
        );
        assert_eq!(pool_frames(2560, 1600, 60), 10, "the test Mac's display");
        assert_eq!(pool_frames(1920, 1080, 30), 15, "half a second fits");
        assert_eq!(pool_frames(1280, 720, 30), 15);
        assert_eq!(pool_frames(3840, 2160, 60), 5);
        assert_eq!(pool_frames(16_384, 16_384, 60), 2, "never fewer than two");
    }

    /// Every step down asks less of the encoder, and the ladder ends at Light and Camera720.
    #[test]
    fn each_lighter_preset_asks_less_of_the_encoder() {
        let load = |id| {
            let p = native_preset(id, Codec::Hevc, (2560, 1600));
            u64::from(p.width * p.height * p.fps)
        };
        for id in [
            PresetId::Native60,
            PresetId::Native30,
            PresetId::Sharp,
            PresetId::SlowMo,
            PresetId::Smooth,
            PresetId::Camera1080,
        ] {
            let next = lighter(id).expect("a lighter preset");
            assert!(load(next) < load(id), "{id:?} to {next:?}");
        }
        assert_eq!(lighter(PresetId::Light), None);
        assert_eq!(lighter(PresetId::Camera720), None);
    }
}
