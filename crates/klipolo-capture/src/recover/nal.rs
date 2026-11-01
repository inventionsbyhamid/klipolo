//! Whether a sample is what an encoder wrote, without decoding it: past the durable length, a power cut can leave a
//! fragment whose box headers landed but whose media is zeros (whole pages of the file that never reached the disk),
//! and the walk, which reads only the boxes, can't tell.
//!
//! H.264 and HEVC samples are NAL units, each after its length (AVCC/HVCC: 1 to 4 bytes, from the `avcC`/`hvcC`).
//! A whole sample is lengths and units that tile it exactly, each unit at least its header long, with a valid header
//! (the forbidden bit clear; H.264's type not 0; HEVC's `nuh_temporal_id_plus1` not 0), a last byte that isn't 0, and
//! never three zero bytes in a row (emulation prevention forbids `00 00 00` inside a unit: H.264 and H.265 §7.4.2).
//! A zeroed page anywhere in a sample breaks one of these. O(sample bytes), no allocation.

use crate::media::{Codec, StreamInfo};

/// `sample` is whole NAL units of `info`'s codec (the module doc).
pub(super) fn whole(sample: &[u8], info: &StreamInfo) -> bool {
    let hevc = info.codec == Codec::Hevc;
    let (prefix, header) = (length_size(info), if hevc { 2 } else { 1 });
    let mut at = 0;
    while at < sample.len() {
        let Some(len) = sample.get(at..at + prefix) else {
            return false;
        };
        let n = len.iter().fold(0usize, |n, &b| (n << 8) | usize::from(b));
        at += prefix;
        let Some(unit) = at.checked_add(n).and_then(|end| sample.get(at..end)) else {
            return false;
        };
        if n < header || !unit_ok(unit, hevc) {
            return false;
        }
        at += n;
    }
    !sample.is_empty()
}

fn unit_ok(unit: &[u8], hevc: bool) -> bool {
    let header = unit[0] & 0x80 == 0
        && if hevc {
            unit[1] & 0x07 != 0
        } else {
            unit[0] & 0x1F != 0
        };
    if !header || unit[unit.len() - 1] == 0 {
        return false;
    }
    let mut zeros = 0;
    for &b in unit {
        zeros = if b == 0 { zeros + 1 } else { 0 };
        if zeros == 3 {
            return false;
        }
    }
    true
}

/// The NAL length's size: `lengthSizeMinusOne` + 1, from byte 4 of an `avcC` body or byte 21 of an `hvcC` body (4
/// when the record is too short to say).
fn length_size(info: &StreamInfo) -> usize {
    let at = match info.codec {
        Codec::H264 => 4,
        Codec::Hevc => 21,
    };
    info.config.get(at).map_or(4, |b| usize::from(b & 0x03) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(codec: Codec) -> StreamInfo {
        StreamInfo {
            codec,
            config: Vec::new(),
            width: 16,
            height: 16,
        }
    }

    #[test]
    fn whole_units_pass_and_zeros_or_a_bad_tiling_fail() {
        let avc = info(Codec::H264);
        let unit = [0x65, 0x88, 0x00, 0x00, 0x03, 0x00, 0x01, 0x80];
        let mut sample = (unit.len() as u32).to_be_bytes().to_vec();
        sample.extend(unit);
        assert!(whole(&sample, &avc));
        let mut zeroed = sample.clone();
        zeroed[6..9].fill(0);
        assert!(!whole(&zeroed, &avc), "a zeroed stretch inside a unit");
        assert!(
            !whole(&sample[..sample.len() - 1], &avc),
            "a unit past the sample"
        );
        assert!(!whole(&[0; 8], &avc), "zero lengths");
        assert!(!whole(&[], &avc));
        let hevc = info(Codec::Hevc);
        assert!(whole(&[0, 0, 0, 3, 0x26, 0x01, 0xAF], &hevc));
        assert!(
            !whole(&[0, 0, 0, 3, 0x26, 0x00, 0xAF], &hevc),
            "temporal id 0"
        );
    }
}
