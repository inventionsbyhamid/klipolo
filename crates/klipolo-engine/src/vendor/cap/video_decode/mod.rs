// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/video-decode/src/lib.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: Windows media_foundation module and its re-exports (incl. FrameTextures) removed.

#[cfg(target_os = "macos")]
pub mod avassetreader;
pub mod ffmpeg;

#[cfg(target_os = "macos")]
pub use avassetreader::{AVAssetReaderDecoder, KeyframeIndex};
pub use ffmpeg::FFmpegDecoder;
