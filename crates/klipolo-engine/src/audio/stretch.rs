//! The live per-stream filter: pitch-preserving speed change (FFmpeg `atempo`, a WSOLA; pattern from Cap
//! `crates/editor/src/audio.rs` `speed_audio_filter`, not vendored) and the item's effect, run in the mixer as the
//! sound plays (under 1% of a core per stream), so the PCM cache never holds a copy per speed or effect. One
//! `atempo` stage covers 0.5–2×, so 0.25× and 4× chain two. Output keeps the input's channel count (mono stays
//! mono: FFmpeg's own upmix would drop it 3 dB); the mixer spreads mono to both sides at unity.

use ffmpeg::filter;
use ffmpeg::format::Sample;
use ffmpeg::format::sample::Type;
use ffmpeg::frame::Audio;
use ffmpeg::{ChannelLayout, Error};

use super::mix::RATE;

const FORMAT: Sample = Sample::F32(Type::Packed);

/// The `atempo` stages whose product is `speed` (each within 0.5–2×).
fn tempo_chain(speed: f64) -> String {
    let mut remaining = speed;
    let mut stages = Vec::new();
    while remaining < 0.5 {
        stages.push("atempo=0.5".to_string());
        remaining /= 0.5;
    }
    while remaining > 2.0 {
        stages.push("atempo=2.0".to_string());
        remaining /= 2.0;
    }
    if (remaining - 1.0).abs() > 1e-9 || stages.is_empty() {
        stages.push(format!("atempo={remaining:.6}"));
    }
    stages.join(",")
}

/// True when `speed` is played as is (no time-stretch).
pub fn is_unit(speed: f64) -> bool {
    (speed - 1.0).abs() < 1e-6
}

/// Streams interleaved 48 kHz samples (`channels` 1 or 2) through the speed change and effect; `push` and `finish`
/// hand back output with the same channel count.
pub struct Stretch {
    graph: filter::Graph,
    channels: usize,
    layout: ChannelLayout,
    pts: i64,
    out: Audio,
}

impl Stretch {
    /// None when there is nothing to do (1× and no effect). `effect` is the effect's FFmpeg filter chain
    /// (`Registry::audio_filter`).
    pub fn new(channels: usize, speed: f64, effect: Option<&str>) -> Result<Option<Self>, Error> {
        if is_unit(speed) && effect.is_none() {
            return Ok(None);
        }
        let (layout, name) = if channels == 1 {
            (ChannelLayout::MONO, "mono")
        } else {
            (ChannelLayout::STEREO, "stereo")
        };
        let mut graph = filter::Graph::new();
        let args = format!(
            "time_base=1/{RATE}:sample_rate={RATE}:sample_fmt={}:channel_layout={name}",
            FORMAT.name()
        );
        graph.add(
            &filter::find("abuffer").ok_or(Error::FilterNotFound)?,
            "in",
            &args,
        )?;
        graph.add(
            &filter::find("abuffersink").ok_or(Error::FilterNotFound)?,
            "out",
            "",
        )?;
        let mut chain: Vec<String> = Vec::new();
        if !is_unit(speed) {
            chain.push(tempo_chain(speed));
        }
        chain.extend(effect.map(str::to_string));
        chain.push(format!(
            "aformat=sample_fmts={}:sample_rates={RATE}:channel_layouts={name}",
            FORMAT.name()
        ));
        graph
            .output("in", 0)?
            .input("out", 0)?
            .parse(&chain.join(","))?;
        graph.validate()?;
        Ok(Some(Self {
            graph,
            channels: channels.max(1),
            layout,
            pts: 0,
            out: Audio::empty(),
        }))
    }

    /// Feeds interleaved samples; output that is ready goes to `emit`.
    pub fn push(&mut self, samples: &[f32], emit: &mut dyn FnMut(&[f32])) -> Result<(), Error> {
        let frames = samples.len() / self.channels;
        if frames == 0 {
            return Ok(());
        }
        let mut frame = Audio::new(FORMAT, frames, self.layout);
        frame.set_rate(RATE);
        frame.set_pts(Some(self.pts));
        self.pts += frames as i64;
        frame.data_mut(0)[..samples.len() * 4].copy_from_slice(bytemuck::cast_slice(samples));
        self.source()?.source().add(&frame)?;
        self.drain(emit)
    }

    /// Flushes the filter's tail.
    pub fn finish(&mut self, emit: &mut dyn FnMut(&[f32])) -> Result<(), Error> {
        self.source()?.source().flush()?;
        self.drain(emit)
    }

    fn source(&mut self) -> Result<filter::Context, Error> {
        self.graph.get("in").ok_or(Error::InvalidData)
    }

    fn drain(&mut self, emit: &mut dyn FnMut(&[f32])) -> Result<(), Error> {
        let mut sink = self.graph.get("out").ok_or(Error::InvalidData)?;
        while sink.sink().frame(&mut self.out).is_ok() {
            let values = self.out.samples() * usize::from(self.out.channels());
            let bytes = &self.out.data(0)[..values * 4];
            // FFmpeg's frame buffers are at least 16-byte aligned, so they read as f32 in place.
            if let Ok(samples) = bytemuck::try_cast_slice::<u8, f32>(bytes) {
                emit(samples);
            }
            // The sink moves a new reference into the frame each time; the old one must be released first.
            // SAFETY: `out` is a valid frame owned by `self`.
            unsafe { ffmpeg::ffi::av_frame_unref(self.out.as_mut_ptr()) };
        }
        Ok(())
    }
}
