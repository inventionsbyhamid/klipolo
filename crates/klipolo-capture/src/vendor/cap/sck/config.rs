// SPDX-License-Identifier: MIT
// MIT (Cap's `scap-screencapturekit`), Copyright (c) 2023-present Cap Software, Inc.
// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/scap-screencapturekit/src/config.rs
// Klipolo changes:
// - `set_fps`/`with_fps` take the minimum frame interval itself (`set_frame_interval`, a `cm::Time`): the pipe asks
//   for 1 / (fps × 1.1) exactly, where Cap truncated 1000 / fps to whole milliseconds;
// - added: `set_pixel_format`, `set_color_matrix` (ITU-R 709-2) and `set_color_space_name` (sRGB), so the buffers are
//   the encoder's `420v` with the colour its BT.709 tags say; `set_excludes_current_process_audio`,
//   `set_sample_rate` and `set_channel_count` (48 kHz stereo); `set_presenter_overlay_alert` (the privacy alert off:
//   the pipe warns itself); `set_scales_to_fit` and `set_preserves_aspect_ratio` (a window's picture);
// - `set_captures_audio` no longer checks for macOS 13: Klipolo's floor is macOS 15;
// - the `workspace-hack` dependency of Cap's crate is gone (Klipolo has none);
// - unused builder methods (`set_src_rect`, `with_src_rect`, `with_width`, `with_height`, `with_shows_cursor`,
//   `with_captures_audio`, `with_queue_depth`) removed.

use cidre::{arc, cf, cm, cv, sc};

pub struct StreamCfgBuilder(arc::R<sc::StreamCfg>);

impl StreamCfgBuilder {
    /// Logical width of the capture area
    pub fn set_width(&mut self, width: usize) {
        self.0.set_width(width);
    }

    /// Logical height of the capture area
    pub fn set_height(&mut self, height: usize) {
        self.0.set_height(height);
    }

    pub fn set_shows_cursor(&mut self, shows_cursor: bool) {
        self.0.set_shows_cursor(shows_cursor);
    }

    /// Klipolo: the shortest time between two frames (Cap's `set_fps` rounded `1000 / fps` down to whole ms).
    pub fn set_frame_interval(&mut self, interval: cm::Time) {
        self.0.set_minimum_frame_interval(interval);
    }

    pub fn set_captures_audio(&mut self, captures_audio: bool) {
        self.0.set_captures_audio(captures_audio);
    }

    /// Sets the queue depth (number of frames to buffer).
    /// Higher values provide more tolerance for processing delays but use more memory.
    /// Apple's default is 3. Maximum is 8.
    pub fn set_queue_depth(&mut self, depth: isize) {
        self.0.set_queue_depth(depth.min(8));
    }

    /// Klipolo: the buffers' pixel format (`420v`: the encoder's NV12, video range).
    pub fn set_pixel_format(&mut self, format: cv::PixelFormat) {
        self.0.set_pixel_format(format);
    }

    /// Klipolo: the YCbCr matrix of a `420v` buffer (ITU-R 709-2, what the encoder's colour tags say).
    pub fn set_color_matrix(&mut self, matrix: &cf::String) {
        self.0.set_color_matrix(matrix);
    }

    /// Klipolo: the colour space the display's pixels are converted to before the matrix (sRGB).
    pub fn set_color_space_name(&mut self, name: &cf::String) {
        self.0.set_color_space_name(name);
    }

    /// Klipolo: leaves this process's own sound out of the stream's.
    pub fn set_excludes_current_process_audio(&mut self, excludes: bool) {
        self.0.set_excludes_current_process_audio(excludes);
    }

    /// Klipolo: the sound's sample rate (48 kHz).
    pub fn set_sample_rate(&mut self, rate: i64) {
        self.0.set_sample_rate(rate);
    }

    /// Klipolo: the sound's channels (2).
    pub fn set_channel_count(&mut self, channels: i64) {
        self.0.set_channel_count(channels);
    }

    /// Klipolo: whether macOS shows its Presenter Overlay privacy alert for this stream (never: the pipe sends its own
    /// warning when the overlay shows in the picture).
    pub fn set_presenter_overlay_alert(&mut self, setting: sc::PresenterOverlayAlertSetting) {
        self.0.set_presenter_overlay_privacy_alert_setting(setting);
    }

    /// Klipolo: content larger or smaller than the output is scaled into it (a window's picture).
    pub fn set_scales_to_fit(&mut self, scales: bool) {
        self.0.set_scales_to_fit(scales);
    }

    /// Klipolo: scaled content keeps its shape.
    pub fn set_preserves_aspect_ratio(&mut self, preserves: bool) {
        self.0.set_preserves_aspect_ratio(preserves);
    }

    pub fn build(self) -> arc::R<sc::StreamCfg> {
        self.0
    }
}

impl Default for StreamCfgBuilder {
    fn default() -> Self {
        Self(sc::StreamCfg::new())
    }
}
