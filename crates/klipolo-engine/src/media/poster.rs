//! A file's poster: the start screen card's picture of a project, kept in the thumbnails folder by the file's key
//! (`thumbs::write_poster`) and made for every project the start screen lists, opened or not. One picture, decoded in
//! software on the caller's thread:
//! - the file's first picture; one that says nothing (`is_dull`: nearly all one colour, such as a black, white or plain
//!   frame) gives way to the picture at 1 s, or a tenth of the way in for a file under 10 s;
//! - fitted in `POSTER_BOX` and never enlarged (`poster_size`), anamorphic pixels widened, turned upright as the file
//!   asks;
//! - in the colours playback shows: its YUV read as BT.709 at the range it says, as the preview's shaders read every
//!   frame, then written in a JPEG's own BT.601 full range;
//! - at JPEG quality about 90.
//!
//! A file without a picture (sound only, or a cover's attached picture) has no poster.

use std::path::Path;

use ffmpeg::format::Pixel;
use ffmpeg::format::stream::Disposition;
use ffmpeg::software::scaling;
use klipolo_model::time::Us;
use klipolo_platform::durable::Steps;

use super::key::{CacheKind, stem};
use super::scan::{next_packet, plain_layout, set_discards, turn};
use super::thumbs::{Poster, poster_in, poster_size, write_poster};
use crate::render::probe::{stream_rotation, stream_start};

/// Where a dull first picture gives way to: 1 s in.
const LATER_US: Us = 1_000_000;
/// A frame this close before the time asked for is the one at it (29.97 fps puts frame 30 at 1.001 s).
const NEAR_US: Us = 1_000;
/// A plane says nothing when this share of its samples is within `DULL_SPREAD` of its median.
const DULL_SHARE: f64 = 0.98;
const DULL_SPREAD: i32 = 12;
/// Packets read with no picture coming out before the file counts as one whose pictures can't be decoded.
const PACKETS_WITHOUT_A_PICTURE: u32 = 600;

/// The time a dull first picture gives way to: 1 s in, or a tenth of the way for a file under 10 s (`duration_us`, 0
/// when it isn't known).
fn later_us(duration_us: Us) -> Us {
    if duration_us > 0 {
        LATER_US.min(duration_us / 10)
    } else {
        LATER_US
    }
}

/// Makes `path`'s poster in the thumbnails folder `root` (replacing an older one), written through `steps`: the poster
/// made, or None for a file without a picture. Fails when the file can't be read or the poster can't be written.
pub fn make_poster(root: &Path, steps: &dyn Steps, path: &str) -> Result<Option<Poster>, String> {
    let _ = ffmpeg::init();
    let key = stem(path, &CacheKind::Thumbs).ok_or("The file isn't there.")?;
    let Some(mut source) = Source::open(path)? else {
        return Ok(None);
    };
    let mut fit = Fit::new(poster_size(source.upright.0, source.upright.1), source.rotation);
    let Some(first_us) = source.next()? else {
        return Ok(None);
    };
    fit.make(&mut source.frame)?;
    let later = later_us(source.duration_us);
    if first_us < later - NEAR_US && fit.dull() {
        // Decoded on from the first picture (at most a second of frames): a seek would land on the same keyframe.
        let mut moved = false;
        while let Some(at) = source.next()? {
            moved = true;
            if at >= later - NEAR_US {
                break;
            }
        }
        // The picture at that time, or the file's last one when it ends sooner.
        if moved {
            fit.make(&mut source.frame)?;
        }
    }
    let (planes, strides) = fit.planes();
    write_poster(steps, root, &key, fit.size, planes, strides)
        .map_err(|e| format!("The poster can't be written: {e}"))?;
    Ok(poster_in(root, &key))
}

/// The file's video decoding: its pictures in presentation order.
struct Source {
    input: ffmpeg::format::context::Input,
    index: usize,
    decoder: ffmpeg::decoder::Video,
    /// Seconds per pts unit, and the stream's first pts.
    seconds: f64,
    start: i64,
    duration_us: Us,
    /// The clockwise turn that shows the stored frames upright, and the picture's size as shown.
    rotation: u16,
    upright: (u32, u32),
    /// The last picture decoded (`next`), and the one being received.
    frame: ffmpeg::frame::Video,
    scratch: ffmpeg::frame::Video,
    ended: bool,
}

impl Source {
    /// None when the file has no picture to show.
    fn open(path: &str) -> Result<Option<Source>, String> {
        let mut input = crate::media::open::input(&path)
            .map_err(|e| format!("The file can't be opened: {e}"))?;
        let Some(stream) = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .filter(|s| !s.disposition().contains(Disposition::ATTACHED_PIC))
        else {
            return Ok(None);
        };
        let (index, tb, start, rotation) = (
            stream.index(),
            stream.time_base(),
            stream_start(&stream),
            stream_rotation(&stream),
        );
        let stream_us = (stream.duration().max(0) as f64 * f64::from(tb) * 1e6) as Us;
        let mut decoder = ffmpeg::codec::Context::from_parameters(stream.parameters())
            .and_then(|c| c.decoder().video())
            .map_err(|e| format!("The picture's decoder can't be opened: {e}"))?;
        decoder.set_packet_time_base(tb);
        set_discards(&mut input, Some(index), None);
        let duration_us = if input.duration() > 0 {
            input.duration()
        } else {
            stream_us
        };
        // The shown shape: anamorphic pixels widened, then the display turn.
        let (w, h) = (decoder.width(), decoder.height());
        let sar = decoder.aspect_ratio();
        let shown_w = if sar.numerator() > 0 && sar.denominator() > 0 {
            (f64::from(w) * f64::from(sar)).round() as u32
        } else {
            w
        };
        let upright = if rotation % 180 == 90 {
            (h, shown_w)
        } else {
            (shown_w, h)
        };
        Ok(Some(Source {
            input,
            index,
            decoder,
            seconds: f64::from(tb),
            start,
            duration_us,
            rotation,
            upright,
            frame: ffmpeg::frame::Video::empty(),
            scratch: ffmpeg::frame::Video::empty(),
            ended: false,
        }))
    }

    /// Decodes the next picture into `frame`: its time, or None at the end of the file (`frame` keeps the last one).
    fn next(&mut self) -> Result<Option<Us>, String> {
        let mut without = 0;
        loop {
            if self.decoder.receive_frame(&mut self.scratch).is_ok() {
                std::mem::swap(&mut self.frame, &mut self.scratch);
                let pts = (self.frame.timestamp())
                    .or_else(|| self.frame.pts())
                    .unwrap_or(self.start);
                return Ok(Some(
                    ((pts - self.start) as f64 * self.seconds * 1e6).round() as Us,
                ));
            }
            if self.ended {
                return Ok(None);
            }
            without += 1;
            if without > PACKETS_WITHOUT_A_PICTURE {
                return Err("No picture in the file could be decoded.".into());
            }
            match next_packet(&mut self.input) {
                // A damaged packet: the decoder resyncs at a later one.
                Ok(Some((stream, packet))) if stream == self.index => {
                    let _ = self.decoder.send_packet(&packet);
                }
                Ok(Some(_)) => {}
                Ok(None) => {
                    self.ended = true;
                    let _ = self.decoder.send_eof();
                }
                Err(e) => return Err(format!("The file can't be read: {e}")),
            }
        }
    }
}

/// The poster as it is made from a decoded picture: scaled (area averaging) to its size in RGB, read as BT.709 as
/// playback reads it, then to a JPEG's 4:2:0 in BT.601 at full range, turned upright, its planes packed row after row.
struct Fit {
    /// As shown (upright).
    size: (u32, u32),
    rotation: u16,
    to_rgb: Option<(scaling::Context, (Pixel, u32, u32, bool))>,
    to_yuv: Option<scaling::Context>,
    rgb: ffmpeg::frame::Video,
    yuv: ffmpeg::frame::Video,
    packed: [Vec<u8>; 3],
}

impl Fit {
    fn new(size: (u32, u32), rotation: u16) -> Self {
        Fit {
            size,
            rotation,
            to_rgb: None,
            to_yuv: None,
            rgb: ffmpeg::frame::Video::empty(),
            yuv: ffmpeg::frame::Video::empty(),
            packed: Default::default(),
        }
    }

    /// The size scaled to before the turn.
    fn stored(&self) -> (u32, u32) {
        let (w, h) = self.size;
        if self.rotation % 180 == 90 {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// Makes the poster of `frame` (a software frame).
    fn make(&mut self, frame: &mut ffmpeg::frame::Video) -> Result<(), String> {
        let (sw, sh) = self.stored();
        // JPEG-style formats (MJPEG's yuvj420p) are their plain layout at full range: swscale warns about the J names.
        let full_range = plain_layout(frame) || frame.color_range() == ffmpeg::color::Range::JPEG;
        let key = (frame.format(), frame.width(), frame.height(), full_range);
        if self.to_rgb.as_ref().is_none_or(|(_, made)| *made != key) {
            let mut scaler = scaling::Context::get(
                key.0,
                key.1,
                key.2,
                Pixel::RGB24,
                sw,
                sh,
                scaling::Flags::AREA,
            )
            .map_err(|e| format!("The poster's scaler: {e}"))?;
            colours(&mut scaler, ffmpeg::ffi::SWS_CS_ITU709, full_range);
            self.to_rgb = Some((scaler, key));
        }
        if self.to_yuv.is_none() {
            let mut scaler = scaling::Context::get(
                Pixel::RGB24,
                sw,
                sh,
                Pixel::YUV420P,
                sw,
                sh,
                scaling::Flags::AREA,
            )
            .map_err(|e| format!("The poster's scaler: {e}"))?;
            colours(&mut scaler, ffmpeg::ffi::SWS_CS_ITU601, true);
            self.to_yuv = Some(scaler);
        }
        let (to_rgb, _) = self.to_rgb.as_mut().expect("made above");
        to_rgb
            .run(frame, &mut self.rgb)
            .map_err(|e| format!("The poster's scaling: {e}"))?;
        let to_yuv = self.to_yuv.as_mut().expect("made above");
        to_yuv
            .run(&self.rgb, &mut self.yuv)
            .map_err(|e| format!("The poster's scaling: {e}"))?;
        for p in 0..3 {
            let (w, h) = if p == 0 { (sw, sh) } else { (sw / 2, sh / 2) };
            let (w, h) = (w as usize, h as usize);
            let (data, stride) = (self.yuv.data(p), self.yuv.stride(p));
            let out = &mut self.packed[p];
            if self.rotation % 360 == 0 {
                out.clear();
                for row in 0..h {
                    out.extend_from_slice(&data[row * stride..row * stride + w]);
                }
            } else {
                turn(data, stride, (w, h), self.rotation % 360, out);
            }
        }
        Ok(())
    }

    /// The poster's planes and their strides.
    fn planes(&self) -> ([&[u8]; 3], [usize; 3]) {
        let w = self.size.0 as usize;
        (
            [&self.packed[0], &self.packed[1], &self.packed[2]],
            [w, w / 2, w / 2],
        )
    }

    fn dull(&self) -> bool {
        let (w, h) = self.size;
        is_dull(self.planes().0, (w as usize, h as usize))
    }
}

/// Makes `scaler` read YUV with `matrix`'s coefficients at full range or not (`full_range_in`), and write full range.
fn colours(scaler: &mut scaling::Context, matrix: i32, full_range_in: bool) {
    // SAFETY: the context is valid for `scaler`'s life; the coefficient table is FFmpeg's own static one.
    unsafe {
        let table = ffmpeg::ffi::sws_getCoefficients(matrix);
        ffmpeg::ffi::sws_setColorspaceDetails(
            scaler.as_mut_ptr(),
            table,
            i32::from(full_range_in),
            table,
            1,
            0,
            1 << 16,
            1 << 16,
        );
    }
}

/// A 4:2:0 picture of `w × h`, its planes packed row after row, says nothing: in each plane nearly every sample
/// (`DULL_SHARE`) is within `DULL_SPREAD` of the plane's median. A black, white or one colour frame, noise over one, or
/// one with a small title; never bars, a page of text or a picture of two colours.
pub(crate) fn is_dull(planes: [&[u8]; 3], (w, h): (usize, usize)) -> bool {
    planes.iter().enumerate().all(|(p, plane)| {
        let count = if p == 0 { w * h } else { (w / 2) * (h / 2) };
        let samples = &plane[..count.min(plane.len())];
        if samples.is_empty() {
            return true;
        }
        let mut histogram = [0usize; 256];
        for &s in samples {
            histogram[usize::from(s)] += 1;
        }
        let mut seen = 0;
        let median = (0..256)
            .find(|&v| {
                seen += histogram[v];
                seen * 2 >= samples.len()
            })
            .unwrap_or(0) as i32;
        let near: usize = (median - DULL_SPREAD..=median + DULL_SPREAD)
            .filter_map(|v| usize::try_from(v).ok())
            .filter(|&v| v < 256)
            .map(|v| histogram[v])
            .sum();
        near as f64 >= DULL_SHARE * samples.len() as f64
    })
}

#[cfg(test)]
mod tests;
