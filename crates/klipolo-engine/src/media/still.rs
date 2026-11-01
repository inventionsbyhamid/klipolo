//! Sprites as JPEG through the encoder seam (`CodecRole::Still`: FFmpeg's own LGPL `mjpeg` on every OS).

use ffmpeg::format::Pixel;

use super::sprite::Sprite;
use crate::platform::encode::CodecRole;

/// FFmpeg's `FF_QP2LAMBDA`: a qscale as the encoder's lambda.
const QP2LAMBDA: usize = 118;
/// JPEG quantiser scale (2 is near lossless, 31 is blocky): small files that still read at 72 px.
const QSCALE: usize = 4;

/// The sprite as one baseline JPEG (about 146 KB for 64 tiles of busy 128 × 72 test video, measured).
pub fn encode_jpeg(sprite: &Sprite) -> Result<Vec<u8>, String> {
    let choice = crate::platform::encode::candidates(CodecRole::Still)
        .first()
        .ok_or("This computer has no picture encoder.")?;
    let codec = ffmpeg::encoder::find_by_name(choice.ffmpeg_name)
        .ok_or("No JPEG encoder in this build.")?;
    let (w, h) = sprite.size();
    let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(|e| format!("JPEG encoder / {e}"))?;
    encoder.set_width(w);
    encoder.set_height(h);
    encoder.set_format(Pixel::YUVJ420P);
    encoder.set_time_base((1, 1));
    encoder.set_quality(QSCALE * QP2LAMBDA);
    encoder.set_flags(ffmpeg::codec::Flags::QSCALE);
    let mut encoder = encoder
        .open_as(codec)
        .map_err(|e| format!("JPEG encoder / {e}"))?;
    let mut frame = ffmpeg::frame::Video::new(Pixel::YUVJ420P, w, h);
    let (planes, strides) = sprite.planes();
    for (p, (src, src_stride)) in planes.into_iter().zip(strides).enumerate() {
        let stride = frame.stride(p);
        let rows = if p == 0 { h } else { h / 2 } as usize;
        let dst = frame.data_mut(p);
        for r in 0..rows {
            dst[r * stride..r * stride + src_stride]
                .copy_from_slice(&src[r * src_stride..(r + 1) * src_stride]);
        }
    }
    frame.set_pts(Some(0));
    encoder
        .send_frame(&frame)
        .and_then(|()| encoder.send_eof())
        .map_err(|e| format!("JPEG encode / {e}"))?;
    let mut packet = ffmpeg::Packet::empty();
    encoder
        .receive_packet(&mut packet)
        .map_err(|e| format!("JPEG encode / {e}"))?;
    Ok(packet.data().map(<[u8]>::to_vec).unwrap_or_default())
}
