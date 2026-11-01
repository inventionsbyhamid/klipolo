//! VideoToolbox's encoder ids, read without asking the OS (`platform::macos` lists the encoders): which ones
//! run apart from the GPU a game uses, and which physical engine each one is. VideoToolbox names them
//! `com.apple.videotoolbox.videoencoder.<codec>.<engine>` (Intel's Quick Sync: `h264.gva`, `hevc.gva`) or
//! `….<engine>.<codec>` (the T2's `ave.hevc`), so a part of the id, not its place, names the engine.

use crate::media::Codec;

/// Whether hardware encoder `encoder_id` runs apart from the GPU on a Mac with `cpu_arch` ("aarch64", "x86_64"): on
/// Apple silicon every hardware encoder is the media engine; on Intel only the T2's AVE is, since Quick Sync (`gva`)
/// shares the iGPU with the game.
pub fn dedicated_on_mac(encoder_id: &str, cpu_arch: &str) -> bool {
    cpu_arch == "aarch64" || has_part(encoder_id, "ave")
}

/// The physical engine hardware encoder `encoder_id` runs on, so a camera and a screen on two ids of one engine share
/// its capacity (`probe::keeps_both`): Apple silicon's media engine for every one (a chip with two
/// counts as one, the safe side), the T2's `ave` or Quick Sync's `gva` on Intel, else the id itself.
pub fn engine_on_mac<'a>(encoder_id: &'a str, cpu_arch: &str) -> &'a str {
    if cpu_arch == "aarch64" {
        return "media";
    }
    ["ave", "gva"]
        .into_iter()
        .find(|engine| has_part(encoder_id, engine))
        .unwrap_or(encoder_id)
}

/// The camera's encoders, in order, when its take names none, from
/// VideoToolbox's list as (id, codec, hardware): H.264 on a shared hardware engine first (Intel's Quick Sync), then a
/// software H.264 encoder, then a dedicated engine last (the T2's AVE, Apple silicon's media engine), which the screen's
/// own pick takes first. So a camera doesn't double up the screen's engine while anything else records, and still
/// starts wherever any H.264 encoder exists (Apple silicon without a software one).
pub fn camera_own_order(encoders: &[(&str, Codec, bool)], cpu_arch: &str) -> Vec<String> {
    let h264 = || {
        encoders
            .iter()
            .filter(|(_, codec, _)| *codec == Codec::H264)
    };
    let dedicated = |id: &str, hardware: bool| hardware && dedicated_on_mac(id, cpu_arch);
    let shared = h264().filter(|(id, _, hardware)| *hardware && !dedicated(id, *hardware));
    let software = h264().filter(|(_, _, hardware)| !hardware);
    let last = h264().filter(|(id, _, hardware)| dedicated(id, *hardware));
    (shared.chain(software).chain(last))
        .map(|(id, _, _)| (*id).to_owned())
        .collect()
}

fn has_part(id: &str, part: &str) -> bool {
    id.split('.').any(|p| p == part)
}
