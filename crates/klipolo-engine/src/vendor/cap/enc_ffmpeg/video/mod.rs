// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/video/mod.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: only `h264` (generalised to the VideoToolbox HEVC and ProRes encoders) and `videotoolbox_hw` kept; `h264_packet`, `hevc` (libx265-first) and `prores` (`prores_ks`) removed.

pub mod h264;
#[cfg(target_os = "macos")]
pub mod videotoolbox_hw;
