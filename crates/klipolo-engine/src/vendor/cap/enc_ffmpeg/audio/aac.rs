// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/audio/aac.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted; the bitrate is a parameter (Klipolo: 192 kb/s; Cap fixed 320 kb/s); `init` prefers Apple's AAC encoder (`aac_at`, AudioToolbox; 16-bit input, constrained VBR) over FFmpeg's native one, which clicked on a loud first frame; `init_pcm24` writes 24-bit PCM (Master .mov) through the same resampler and base; `initial_padding`; the unit tests removed.

use std::{thread, time::Duration};

use super::super::super::media_info::{AudioInfo, FFRational};
use ffmpeg::{
    codec::{context, encoder},
    format::{self, Sample, sample::Type},
    frame,
    threading::Config,
};

use super::{AudioEncoder, base::AudioEncoderBase, buffered_resampler::BufferedResampler};

#[derive(thiserror::Error, Debug)]
pub enum AACEncoderError {
    #[error("{0:?}")]
    FFmpeg(#[from] ffmpeg::Error),
    #[error("AAC codec not found")]
    CodecNotFound,
    #[error("Sample rate not supported: {0}")]
    RateNotSupported(i32),
    #[error("Resampler: {0}")]
    Resampler(ffmpeg::Error),
}

pub struct AACEncoder {
    base: AudioEncoderBase,
}

impl AACEncoder {
    const SAMPLE_FORMAT: Sample = Sample::F32(Type::Planar);

    pub fn factory(
        input_config: AudioInfo,
        bitrate: usize,
    ) -> impl FnOnce(&mut format::context::Output) -> Result<Self, AACEncoderError> {
        move |o| Self::init(input_config, bitrate, o)
    }

    pub fn init(
        input_config: AudioInfo,
        bitrate: usize,
        output: &mut format::context::Output,
    ) -> Result<Self, AACEncoderError> {
        // Klipolo: Apple's AAC-LC encoder when the build has AudioToolbox (macOS), else FFmpeg's.
        if encoder::find_by_name("aac_at").is_some() {
            let mut options = ffmpeg::Dictionary::new();
            options.set("aac_at_mode", "cvbr");
            options.set("aac_at_quality", "0");
            let at = Self::init_codec(
                "aac_at",
                Sample::I16(Type::Packed),
                bitrate,
                options,
                input_config,
                output,
            );
            if at.is_ok() {
                return at;
            }
        }
        let options = ffmpeg::Dictionary::new();
        Self::init_codec(
            "aac",
            Self::SAMPLE_FORMAT,
            bitrate,
            options,
            input_config,
            output,
        )
    }

    /// Klipolo: 24-bit PCM (`pcm_s24le` takes 32-bit samples and keeps the top 24 bits).
    pub fn init_pcm24(
        input_config: AudioInfo,
        output: &mut format::context::Output,
    ) -> Result<Self, AACEncoderError> {
        Self::init_codec(
            "pcm_s24le",
            Sample::I32(Type::Packed),
            0,
            ffmpeg::Dictionary::new(),
            input_config,
            output,
        )
    }

    fn init_codec(
        name: &str,
        sample_format: Sample,
        bitrate: usize,
        options: ffmpeg::Dictionary<'static>,
        input_config: AudioInfo,
        output: &mut format::context::Output,
    ) -> Result<Self, AACEncoderError> {
        let codec = encoder::find_by_name(name).ok_or(AACEncoderError::CodecNotFound)?;
        let mut encoder_ctx = context::Context::new_with_codec(codec);
        let thread_count = thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(1);
        encoder_ctx.set_threading(Config::count(thread_count));
        let mut encoder = encoder_ctx.encoder().audio()?;

        let rate = {
            let mut rates = codec
                .audio()
                .ok()
                .and_then(|a| a.rates())
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            rates.sort();
            // Klipolo: PCM lists no rates (any rate).
            if rates.is_empty() {
                input_config.rate()
            } else {
                let Some(&rate) = rates
                    .iter()
                    .find(|r| **r >= input_config.rate())
                    .or(rates.first())
                else {
                    return Err(AACEncoderError::RateNotSupported(input_config.rate()));
                };
                rate
            }
        };

        let mut output_config = input_config;
        output_config.sample_format = sample_format;
        output_config.sample_rate = rate as u32;

        let resampler = BufferedResampler::new(input_config, output_config)
            .map_err(AACEncoderError::Resampler)?;

        if bitrate > 0 {
            encoder.set_bit_rate(bitrate);
        }
        encoder.set_rate(rate);
        encoder.set_format(output_config.sample_format);
        encoder.set_channel_layout(output_config.channel_layout());
        encoder.set_time_base(FFRational(1, output_config.rate()));

        let encoder = encoder.open_with(options)?;

        let mut output_stream = output.add_stream(codec)?;
        output_stream.set_time_base(FFRational(1, output_config.rate()));
        output_stream.set_parameters(&encoder);

        Ok(Self {
            base: AudioEncoderBase::new(encoder, resampler, output_stream.index()),
        })
    }

    pub fn send_frame(
        &mut self,
        frame: frame::Audio,
        timestamp: Duration,
        output: &mut format::context::Output,
    ) -> Result<(), ffmpeg::Error> {
        self.base.send_frame(frame, timestamp, output)
    }

    pub fn flush(&mut self, output: &mut format::context::Output) -> Result<(), ffmpeg::Error> {
        self.base.flush(output)
    }

    /// Klipolo: priming samples at the start of the encoded stream (Apple's AAC: 2112; PCM: 0).
    pub fn initial_padding(&self) -> usize {
        self.base.initial_padding()
    }
}

impl AudioEncoder for AACEncoder {
    fn send_frame(&mut self, frame: frame::Audio, output: &mut format::context::Output) {
        let _ = self.send_frame(frame, Duration::MAX, output);
    }

    fn try_send_frame(
        &mut self,
        frame: frame::Audio,
        output: &mut format::context::Output,
    ) -> Result<(), ffmpeg::Error> {
        self.send_frame(frame, Duration::MAX, output)
    }

    fn flush(&mut self, output: &mut format::context::Output) -> Result<(), ffmpeg::Error> {
        self.flush(output)
    }
}
