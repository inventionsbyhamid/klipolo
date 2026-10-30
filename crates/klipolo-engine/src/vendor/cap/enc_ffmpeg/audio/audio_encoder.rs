// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/audio/audio_encoder.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: none (unchanged apart from this header).

use ffmpeg::{format, frame};

pub trait AudioEncoder {
    fn boxed(self) -> Box<dyn AudioEncoder + Send + 'static>
    where
        Self: Send + Sized + 'static,
    {
        Box::new(self)
    }

    fn send_frame(&mut self, frame: frame::Audio, output: &mut format::context::Output);
    fn try_send_frame(
        &mut self,
        frame: frame::Audio,
        output: &mut format::context::Output,
    ) -> Result<(), ffmpeg::Error>;
    fn flush(&mut self, output: &mut format::context::Output) -> Result<(), ffmpeg::Error>;
}
