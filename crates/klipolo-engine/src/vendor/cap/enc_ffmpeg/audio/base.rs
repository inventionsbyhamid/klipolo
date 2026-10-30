// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/audio/base.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted; an encoder without a fixed frame size (PCM) is fed 1024-sample frames; `initial_padding` (the encoder's priming samples).

use super::super::base::EncoderBase;
use super::buffered_resampler::BufferedResampler;
use ffmpeg::{codec::encoder, format, frame};
use std::time::Duration;

pub struct AudioEncoderBase {
    inner: EncoderBase,
    encoder: encoder::Audio,
    resampler: BufferedResampler,
}

impl AudioEncoderBase {
    pub fn new(encoder: encoder::Audio, resampler: BufferedResampler, stream_index: usize) -> Self {
        Self {
            inner: EncoderBase::new(stream_index),
            encoder,
            resampler,
        }
    }

    pub fn send_frame(
        &mut self,
        mut frame: frame::Audio,
        timestamp: Duration,
        output: &mut format::context::Output,
    ) -> Result<(), ffmpeg::Error> {
        // Input frames are stamped in input-rate units; BufferedResampler
        // rescales them to the encoder's output rate.
        let input_rate = f64::from(self.resampler.input().rate);
        self.inner
            .update_pts_with_rate(&mut frame, timestamp, input_rate);

        self.resampler.add_frame(frame);

        while let Some(frame) = self.resampler.get_frame(self.frame_size()) {
            self.inner.send_frame(&frame, output, &mut self.encoder)?;
        }

        Ok(())
    }

    /// Klipolo: the encoder's priming (samples its output starts with before the input's first sample; 0 for PCM).
    pub fn initial_padding(&self) -> usize {
        unsafe { (*self.encoder.as_ptr()).initial_padding.max(0) as usize }
    }

    /// Klipolo: PCM encoders report 0 (any size); they get 1024-sample frames.
    fn frame_size(&self) -> usize {
        match self.encoder.frame_size() {
            0 => 1024,
            n => n as usize,
        }
    }

    pub fn flush(&mut self, output: &mut format::context::Output) -> Result<(), ffmpeg::Error> {
        while let Some(frame) = self.resampler.flush(self.frame_size()) {
            self.inner.send_frame(&frame, output, &mut self.encoder)?;
        }

        self.inner.process_eof(output, &mut self.encoder)
    }
}
