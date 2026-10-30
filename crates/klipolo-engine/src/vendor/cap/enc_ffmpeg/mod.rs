// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/lib.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: the crate root became a module; only `base`, `video` (h264, videotoolbox_hw) and `audio` (aac) kept; `mux` (mp4: export writes its own segment and joined files), `remux`, `segmented_input`, `relocatable_source`, `file_sync`, the fragmented, segmented, DASH, Ogg and MOV muxers, `hevc`, `prores` and `opus` removed.

mod base;
pub use base::{EncodedPacket, EncodedPacketStats};

mod audio;
pub use audio::*;

mod video;
pub use video::*;
