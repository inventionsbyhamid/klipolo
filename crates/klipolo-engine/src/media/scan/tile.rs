//! A scan's video decoder, and its decoded keyframes as tiles: downloaded from the GPU when the hardware decoder made
//! them, scaled (area averaging) straight to the tile as full-range 4:2:0, then turned upright. CPU work on a 72 px
//! tile, about once per second of video. The file's first picture is scaled once more, to its poster.

use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;

use crate::media::keyframes::StoredKeys;
use crate::media::sprite::tile_size;
use crate::media::thumbs::poster_size;
use crate::vendor::cap::ffmpeg_hw_device::{CodecContextExt, HwDevice};
use klipolo_model::time::Us;

/// A picture's Y, U and V planes and their rows' strides.
pub(super) type Planes<'a> = ([&'a [u8]; 3], [usize; 3]);

pub(super) struct Video {
    /// Every frame is a keyframe by the codec's nature (ProRes, MJPEG, DNxHD…): scans seek once per slot instead of
    /// reading every frame.
    pub(super) intra_only: bool,
    pub(super) decoder: ffmpeg::decoder::Video,
    hw: Option<HwDevice>,
    /// To reopen the decoder in software when the hardware one can't decode the stream.
    params: ffmpeg::codec::Parameters,
    packet_time_base: ffmpeg::Rational,
    /// Seconds per pts unit, and the stream's first pts.
    time_base: f64,
    start: i64,
    /// The tile and the poster as shown, and the clockwise turn that shows the stored frames upright.
    tile: Fit,
    poster: Fit,
    rotation: u16,
    pub(super) frame: ffmpeg::frame::Video,
}

/// One size the decoded frames are scaled to (as shown, upright): its scaler and buffers.
struct Fit {
    size: (u32, u32),
    scaler: Option<scaling::Context>,
    scaled: ffmpeg::frame::Video,
    turned: [Vec<u8>; 3],
}

impl Fit {
    fn new(size: (u32, u32)) -> Self {
        Fit {
            size,
            scaler: None,
            scaled: ffmpeg::frame::Video::empty(),
            turned: Default::default(),
        }
    }

    /// `src` scaled (area averaging) to this size as full-range 4:2:0 and turned upright by `rotation` degrees
    /// clockwise: its planes and strides.
    fn planes(
        &mut self,
        src: &ffmpeg::frame::Video,
        full_range_in: bool,
        rotation: u16,
    ) -> Option<Planes<'_>> {
        let (tw, th) = self.size;
        let (sw, sh) = if rotation % 180 == 90 {
            (th, tw)
        } else {
            (tw, th)
        };
        let fits = self.scaler.as_ref().is_some_and(|s| {
            let i = s.input();
            (i.format, i.width, i.height) == (src.format(), src.width(), src.height())
        });
        if !fits {
            let mut scaler = scaling::Context::get(
                src.format(),
                src.width(),
                src.height(),
                Pixel::YUV420P,
                sw,
                sh,
                scaling::Flags::AREA,
            )
            .ok()?;
            full_range_out(&mut scaler, full_range_in);
            self.scaler = Some(scaler);
        }
        self.scaler.as_mut()?.run(src, &mut self.scaled).ok()?;
        if rotation == 0 {
            let s = &self.scaled;
            return Some((
                [s.data(0), s.data(1), s.data(2)],
                [s.stride(0), s.stride(1), s.stride(2)],
            ));
        }
        for p in 0..3 {
            let (w, h) = if p == 0 { (sw, sh) } else { (sw / 2, sh / 2) };
            turn(
                self.scaled.data(p),
                self.scaled.stride(p),
                (w as usize, h as usize),
                rotation,
                &mut self.turned[p],
            );
        }
        let w = tw as usize;
        Some((
            [&self.turned[0], &self.turned[1], &self.turned[2]],
            [w, w / 2, w / 2],
        ))
    }
}

impl Video {
    /// Opens stream `index`'s decoder (on the hardware decoder where there is one), and reads what the keyframe index
    /// keeps about the stream (as `KeyframeIndex::build` reads it).
    pub(super) fn open(
        input: &ffmpeg::format::context::Input,
        index: usize,
    ) -> Result<(Video, StoredKeys), String> {
        let stream = input.stream(index).ok_or("no such stream")?;
        let mut decoder = ffmpeg::codec::Context::from_parameters(stream.parameters())
            .and_then(|c| c.decoder().video())
            .map_err(|e| format!("thumbnail decoder: {e}"))?;
        let tb = stream.time_base();
        decoder.set_packet_time_base(tb);
        // SAFETY: FFmpeg's static descriptor table entry for the codec, or null.
        let intra_only = unsafe {
            let d = ffmpeg::ffi::avcodec_descriptor_get(stream.parameters().id().into());
            !d.is_null() && (*d).props & ffmpeg::ffi::AV_CODEC_PROP_INTRA_ONLY != 0
        };
        let (w, h) = (decoder.width(), decoder.height());
        let fps = stream.avg_frame_rate();
        let start = crate::render::probe::stream_start(&stream);
        let meta = StoredKeys {
            pts: Vec::new(),
            time_base: (tb.numerator(), tb.denominator()),
            start,
            fps: if fps.denominator() == 0 {
                30.0
            } else {
                f64::from(fps.numerator()) / f64::from(fps.denominator())
            },
            duration_secs: if stream.duration() > 0 {
                stream.duration() as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
            } else {
                0.0
            },
            pixel_format: decoder
                .format()
                .descriptor()
                .map_or("", |d| d.name())
                .to_owned(),
            codec: stream.parameters().id().name().to_owned(),
            width: w,
            height: h,
        };
        let hw = crate::platform::decode::hw_device_type()
            .and_then(|kind| decoder.try_use_hw_device(kind).ok());
        // Read off the open stream: the file is never probed again for it.
        let rotation = crate::render::probe::stream_rotation(&stream);
        // The shown shape: anamorphic pixels widened, then the display turn.
        let sar = decoder.aspect_ratio();
        let shown_w = if sar.numerator() > 0 && sar.denominator() > 0 {
            (f64::from(w) * f64::from(sar)).round() as u32
        } else {
            w
        };
        let (upright_w, upright_h) = if rotation % 180 == 90 {
            (h, shown_w)
        } else {
            (shown_w, h)
        };
        let video = Video {
            intra_only,
            decoder,
            hw,
            params: stream.parameters().clone(),
            packet_time_base: tb,
            time_base: f64::from(tb),
            start,
            tile: Fit::new(tile_size(upright_w, upright_h)),
            poster: Fit::new(poster_size(upright_w, upright_h)),
            rotation,
            frame: ffmpeg::frame::Video::empty(),
        };
        Ok((video, meta))
    }

    /// Source time of a pts of this stream.
    pub(super) fn us(&self, pts: i64) -> Us {
        ((pts - self.start) as f64 * self.time_base * 1e6).round() as Us
    }

    /// Sends one packet. A hardware decoder that refuses it (a profile the media engine lacks) is replaced by a
    /// software one, which decodes from here on.
    pub(super) fn send(&mut self, packet: &ffmpeg::Packet) -> Result<(), ffmpeg::Error> {
        match self.decoder.send_packet(packet) {
            Err(_) if self.hw.is_some() => {
                self.hw = None;
                let mut decoder = ffmpeg::codec::Context::from_parameters(self.params.clone())
                    .and_then(|c| c.decoder().video())?;
                decoder.set_packet_time_base(self.packet_time_base);
                self.decoder = decoder;
                self.decoder.send_packet(packet)
            }
            sent => sent,
        }
    }

    /// The tile as shown.
    pub(super) fn tile(&self) -> (u32, u32) {
        self.tile.size
    }

    /// The last decoded frame as a tile (its planes and strides).
    pub(super) fn tile_planes(&mut self) -> Option<Planes<'_>> {
        self.planes(false)
    }

    /// The last decoded frame as the poster: its size, planes and strides.
    pub(super) fn poster_planes(&mut self) -> Option<((u32, u32), Planes<'_>)> {
        let size = self.poster.size;
        Some((size, self.planes(true)?))
    }

    /// The last decoded frame scaled to the tile, or to the poster.
    fn planes(&mut self, poster: bool) -> Option<Planes<'_>> {
        let mut downloaded = self.hw.as_ref().and_then(|hw| hw.get_hwframe(&self.frame));
        let src = match downloaded.as_mut() {
            Some(frame) => frame,
            None => &mut self.frame,
        };
        // JPEG-style formats (MJPEG's yuvj420p) are their plain layout at full range: swscale warns about the J names.
        let full_range_in = plain_layout(src) || src.color_range() == ffmpeg::color::Range::JPEG;
        let fit = if poster {
            &mut self.poster
        } else {
            &mut self.tile
        };
        fit.planes(src, full_range_in, self.rotation)
    }
}

/// Relabels a JPEG-style frame (`yuvj4xxp`) as its plain layout at full range, the same bytes; true when it was one
/// (the frame then says full range, so the poster's scaling of the same frame reads it as full range too).
pub(crate) fn plain_layout(frame: &mut ffmpeg::frame::Video) -> bool {
    let plain = match frame.format() {
        Pixel::YUVJ420P => Pixel::YUV420P,
        Pixel::YUVJ422P => Pixel::YUV422P,
        Pixel::YUVJ444P => Pixel::YUV444P,
        Pixel::YUVJ440P => Pixel::YUV440P,
        Pixel::YUVJ411P => Pixel::YUV411P,
        _ => return false,
    };
    // SAFETY: plain fields of the frame; the J and plain formats share their planes' layout, and J means full range.
    unsafe {
        let raw = frame.as_mut_ptr();
        (*raw).format = ffmpeg::ffi::AVPixelFormat::from(plain) as i32;
        (*raw).color_range = ffmpeg::ffi::AVColorRange::AVCOL_RANGE_JPEG;
    }
    true
}

/// Makes `scaler` write full-range (JPEG) 4:2:0, the sprite's format (asking for YUVJ420P works the same, but FFmpeg
/// warns that it is deprecated).
fn full_range_out(scaler: &mut scaling::Context, full_range_in: bool) {
    use ffmpeg::ffi::{sws_getColorspaceDetails, sws_setColorspaceDetails};
    let (mut inv_table, mut table) = (std::ptr::null_mut(), std::ptr::null_mut());
    let (mut src_range, mut dst_range) = (0, 0);
    let (mut brightness, mut contrast, mut saturation) = (0, 0, 0);
    // SAFETY: the context is valid for `scaler`'s life; FFmpeg fills the out-parameters with pointers into its own
    // tables, which `sws_setColorspaceDetails` takes back unchanged.
    unsafe {
        let ctx = scaler.as_mut_ptr();
        if sws_getColorspaceDetails(
            ctx,
            &mut inv_table,
            &mut src_range,
            &mut table,
            &mut dst_range,
            &mut brightness,
            &mut contrast,
            &mut saturation,
        ) >= 0
        {
            sws_setColorspaceDetails(
                ctx,
                inv_table,
                i32::from(full_range_in),
                table,
                1,
                brightness,
                contrast,
                saturation,
            );
        }
    }
}

/// Turns a `w × h` plane clockwise by `degrees` (90, 180 or 270) into `out`, tightly packed.
fn turn(src: &[u8], stride: usize, (w, h): (usize, usize), degrees: u16, out: &mut Vec<u8>) {
    out.clear();
    out.resize(w * h, 0);
    let at = |x: usize, y: usize| src[y * stride + x];
    match degrees {
        // Out is h wide and w tall.
        90 => {
            for y in 0..w {
                for x in 0..h {
                    out[y * h + x] = at(y, h - 1 - x);
                }
            }
        }
        180 => {
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = at(w - 1 - x, h - 1 - y);
                }
            }
        }
        _ => {
            for y in 0..w {
                for x in 0..h {
                    out[y * h + x] = at(w - 1 - y, x);
                }
            }
        }
    }
}
