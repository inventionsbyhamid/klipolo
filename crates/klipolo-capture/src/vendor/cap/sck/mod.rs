// SPDX-License-Identifier: MIT
// MIT (Cap's `scap-screencapturekit`), Copyright (c) 2023-present Cap Software, Inc.
// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/scap-screencapturekit/src/lib.rs
// Klipolo changes: only the two modules Klipolo takes and their exports; `permission` and
// `is_system_audio_supported` are left out (Klipolo reads the permission without a prompt in `platform::macos`, and its
// macOS floor is 15).

mod capture;
mod config;

pub(crate) use capture::{Capturer, Frame};
pub(crate) use config::StreamCfgBuilder;
