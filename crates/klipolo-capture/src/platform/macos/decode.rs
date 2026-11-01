//! The 10 s read-back's decode (`Backend::decodes`): a `VTDecompressionSession` made from the run's own
//! stream description (its `avcC`/`hvcC`, as the file's head carries it) decodes the run's first keyframe, and the
//! answer is whether a picture came out. A take whose encoder writes what no decoder reads is caught ten seconds in,
//! not at the edit. Once per take; nothing captured, nothing written.

use std::ffi::c_void;

use cidre::{cf, cm, cv, os, vt};

use super::encoders::{guarded, video_codec};
use crate::media::{Codec, StreamInfo};

/// Whether `keyframe` (an AVCC sample) decodes to a picture with `info`'s description; false for a description or a
/// sample VideoToolbox can't take. Always an answer on macOS.
pub fn decodes(info: &StreamInfo, keyframe: &[u8]) -> Option<bool> {
    Some(decode_one(info, keyframe).unwrap_or(false))
}

fn decode_one(info: &StreamInfo, keyframe: &[u8]) -> Option<bool> {
    let atom = match info.codec {
        Codec::H264 => "avcC",
        Codec::Hevc => "hvcC",
    };
    let (name, config) = (
        cf::String::from_str(atom),
        cf::Data::from_slice(&info.config)?,
    );
    let atoms = cf::Dictionary::with_keys_values(&[name.as_ref()], &[config.as_ref()])?;
    let extensions = cf::DictionaryOf::with_keys_values(
        &[cm::FormatDescExtKey::sample_desc_ext_atoms()],
        &[atoms.as_type_ref()],
    );
    let (w, h) = (
        i32::try_from(info.width).ok()?,
        i32::try_from(info.height).ok()?,
    );
    let desc = cm::VideoFormatDesc::video(video_codec(info.codec), w, h, Some(&extensions)).ok()?;
    let sample = sample_of(keyframe, &desc)?;
    let mut decoded = false;
    let record = vt::decompression::OutputCbRecord::new(&mut decoded as *mut bool, Some(picture));
    let made = guarded("VTDecompressionSessionCreate", || {
        vt::DecompressionSession::new(&desc, None, None, Some(&record))
    });
    let mut session = made.ok()?;
    let ran = guarded("VTDecompressionSessionDecodeFrame", || {
        session.decode(&sample, vt::DecodeFrameFlags::default())
    });
    let _ = session.wait_for_async_frames();
    session.invalidate();
    Some(ran.is_ok() && decoded)
}

/// One sample of `bytes` with `desc`, at time 0.
fn sample_of(bytes: &[u8], desc: &cm::FormatDesc) -> Option<cidre::arc::R<cm::SampleBuf>> {
    let mut block = cm::BlockBuf::with_mem_block(bytes.len().max(1), None).ok()?;
    block.as_mut_slice().ok()?[..bytes.len()].copy_from_slice(bytes);
    let timing = cm::SampleTimingInfo {
        duration: cm::Time::invalid(),
        pts: cm::Time::zero(),
        dts: cm::Time::invalid(),
    };
    let size = bytes.len();
    let mut out = None;
    // SAFETY: one sample, with its timing and size, over a block buffer that holds it.
    unsafe {
        cm::SampleBuf::create_in(
            None,
            Some(&block),
            true,
            None,
            std::ptr::null(),
            Some(desc),
            1,
            1,
            &timing,
            1,
            &size,
            &mut out,
        )
        .ok()?;
    }
    out
}

/// A picture came out (status 0 and an image).
extern "C" fn picture(
    decoded: *mut bool,
    _frame: *mut c_void,
    status: os::Status,
    _flags: vt::DecodeInfoFlags,
    image: Option<&cv::ImageBuf>,
    _pts: cm::Time,
    _duration: cm::Time,
) {
    if status.0 == 0 && image.is_some() {
        // SAFETY: `decode_one`'s flag, alive until its session is invalidated.
        unsafe { *decoded = true };
    }
}
