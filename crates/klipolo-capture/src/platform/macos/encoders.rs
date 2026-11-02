//! VideoToolbox's hardware H.264 and HEVC encoders (`VTCopyVideoEncoderList`): a list, no session and no capture. The
//! probe measures each, and a take pins the one it chose by its id: on a 2018-2020 Intel Mac with a T2 that is the
//! T2's `ave` HEVC beside Quick Sync's `gva` H.264, which "require hardware" alone wouldn't tell apart.

use cidre::{cf, cm, ns, os, vt};

use crate::media::Codec;

/// A hardware encoder: its VideoToolbox id (`EncoderSpec::encoder_id`) and codec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderInfo {
    pub id: String,
    pub codec: Codec,
}

/// The hardware H.264 and HEVC encoders, in VideoToolbox's order; empty when the list can't be read.
pub fn hardware_encoders() -> Vec<EncoderInfo> {
    (listed().into_iter())
        .filter_map(|(info, hardware)| hardware.then_some(info))
        .collect()
}

/// Every H.264 and HEVC encoder VideoToolbox lists, hardware or software, with whether it is hardware.
pub(super) fn listed() -> Vec<(EncoderInfo, bool)> {
    let Ok(list) = vt::video_encoder_list::copy() else {
        tracing::warn!("encoders: VideoToolbox's list couldn't be read");
        return vec![];
    };
    // SAFETY: VideoToolbox's constant keys, valid for the process's life.
    let (id_key, codec_key, hardware_key) = unsafe {
        (
            kVTVideoEncoderList_EncoderID,
            kVTVideoEncoderList_CodecType,
            kVTVideoEncoderList_IsHardwareAccelerated,
        )
    };
    list.iter()
        .filter_map(|entry| {
            // SAFETY: each entry is a dictionary of string keys to property-list values (`VTVideoEncoderList.h`).
            let entry: &cf::DictionaryOf<cf::String, cf::Plist> =
                unsafe { std::mem::transmute(entry) };
            let hardware = (entry.value(hardware_key))
                .and_then(cf::Plist::try_as_boolean)
                .is_some_and(cf::Boolean::value);
            let codec = (entry.value(codec_key))
                .and_then(cf::Plist::try_as_number)
                .and_then(cf::Number::to_i64)
                .and_then(|c| codec_of(c as u32))?;
            let id = entry.value(id_key)?.try_as_string()?.to_string();
            Some((EncoderInfo { id, codec }, hardware))
        })
        .collect()
}

/// The list's `CodecType`, a four-character code.
fn codec_of(code: u32) -> Option<Codec> {
    match &code.to_be_bytes() {
        b"avc1" => Some(Codec::H264),
        b"hvc1" => Some(Codec::Hevc),
        _ => None,
    }
}

/// VideoToolbox's codec type for `codec`.
pub(super) fn video_codec(codec: Codec) -> cm::VideoCodec {
    match codec {
        Codec::H264 => cm::VideoCodec::H264,
        Codec::Hevc => cm::VideoCodec::HEVC,
    }
}

/// The encoder specification that pins `encoder_id` (`kVTVideoEncoderSpecification_EncoderID`).
pub(super) fn pinned(encoder_id: &str) -> Option<cidre::arc::R<cf::Dictionary>> {
    let id = cf::String::from_str(encoder_id);
    // SAFETY: a VideoToolbox constant key.
    let key = unsafe { kVTVideoEncoderSpecification_EncoderID };
    cf::Dictionary::with_keys_values(&[key.as_ref()], &[id.as_ref()])
}

/// A VideoToolbox call that may throw an Objective-C exception instead of returning its status: on macOS 15.7.5 the
/// HEVC encoder threw one inside `prepare` (`-[__NSCFBoolean count]`), and one crossing Rust aborts the worker. Thrown,
/// it is `COULD_NOT_CREATE_INSTANCE` (−12907), logged with its reason. For setup calls, not the frame path.
pub(super) fn guarded<R>(what: &str, call: impl FnOnce() -> os::Result<R>) -> os::Result<R> {
    match ns::try_catch(call) {
        Ok(result) => result,
        Err(thrown) => {
            let reason = thrown.reason().map(|r| r.to_string());
            tracing::warn!(what, ?reason, "encode: VideoToolbox threw");
            Err(vt::errors::COULD_NOT_CREATE_INSTANCE)
        }
    }
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    static kVTVideoEncoderList_EncoderID: &'static cf::String;
    static kVTVideoEncoderList_CodecType: &'static cf::String;
    static kVTVideoEncoderList_IsHardwareAccelerated: &'static cf::String;
    static kVTVideoEncoderSpecification_EncoderID: &'static cf::String;
}
