// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/audio/mod.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: `opus` removed.

mod audio_encoder;
pub use audio_encoder::*;

mod base;

pub mod buffered_resampler;

pub mod aac;
